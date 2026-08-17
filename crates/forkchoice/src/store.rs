//! Forkchoice store: blocks, post-states, votes, and the canonical clock.
//!
//! [`Store`] tracks every block + post-state pair received by the node, plus
//! the validator vote maps used by LMD-GHOST. This revision exposes only the
//! data container, the `from_anchor` constructor, and minimal read accessors.
//! Block-add, attestation processing, and head resolution land in subsequent
//! issues in this part.
//!
//! # `block_order` invariant
//!
//! `block_order` records the order in which roots first entered `blocks`.
//! Re-inserting a known root is a no-op for both the maps and the order
//! vector. The vector is **first-seen order, not slot-sorted** — the
//! canonical tie-break for head resolution operates on `(weight, root)`
//! and never consults `block_order` or the block slot.
//!
//! # Vote-map cost note
//!
//! `latest_known_votes` and `latest_new_votes` hold [`SignedAttestation`] values
//! by-value. Each [`SignedAttestation`] carries a 3116-byte signature container,
//! so the maps grow by ≈3.2 KB per validator. Vote-pool churn happens
//! through [`Store::process_attestation`] and the phase hooks.

use std::collections::{hash_map::Entry, HashMap};
use std::sync::Arc;

use config::{INTERVALS_PER_SLOT, JUSTIFICATION_LOOKBACK_SLOTS};
use protocol::{
    AttestationData, Block, Checkpoint, ProtocolConfig, SignedAttestation, Slot, State,
    ValidatorIndex,
};
use ssz::HashTreeRoot;
use types::Bytes32;

use crate::error::ForkchoiceError;
use crate::helpers::get_fork_choice_head;
use crate::time::{Phase, Time};

/// LMD-GHOST forkchoice store.
///
/// Mirrors the data shape of upstream `consensus/forkchoice.Store` while
/// staying idiomatic to Rust (private fields, `&`-returning accessors, no
/// `defer FuncTrace`). All inputs are owned; [`Self::from_anchor`] takes
/// `state` and `anchor_block` by value to avoid the implicit clone they
/// would require to enter the maps.
///
/// # `Default`
///
/// [`Store::default()`] returns an all-zero structural placeholder: empty
/// maps, zero-root `head` and `safe_target`, zero-slot `latest_justified` /
/// `latest_finalized`, and `time = 0`. It is **not** a usable fork-choice
/// state — every accessor that resolves a root will return `None` because
/// the zero root is not tracked. Use [`Self::from_anchor`] for real
/// construction. The `Default` impl exists for struct-update syntax,
/// `mem::take`, and test scaffolding.
#[derive(Debug, Clone, Default)]
pub struct Store {
    time: Time,
    config: ProtocolConfig,
    head: Bytes32,
    safe_target: Bytes32,
    latest_justified: Checkpoint,
    latest_finalized: Checkpoint,
    blocks: HashMap<Bytes32, Block>,
    // Post-states are `Arc`-wrapped so the hot import path can capture a
    // post-state for persistence with a refcount bump instead of a deep clone
    // under the engine mutex (see lean-chain `capture_persist_plan`).
    states: HashMap<Bytes32, Arc<State>>,
    block_order: Vec<Bytes32>,
    latest_known_votes: HashMap<ValidatorIndex, SignedAttestation>,
    latest_new_votes: HashMap<ValidatorIndex, SignedAttestation>,
}

impl Store {
    /// Builds a fresh forkchoice store from a trusted anchor pair.
    ///
    /// `anchor_block.state_root` must equal `hash_tree_root(state)`. The
    /// anchor's clock value is `anchor_block.slot * INTERVALS_PER_SLOT`. On
    /// success `state` and `anchor_block` are moved into the store; the
    /// `block_order` vector is seeded with the single anchor root, and both
    /// `head` and `safe_target` point at the anchor. A zero genesis
    /// justified/finalized checkpoint is normalized to the anchor root so
    /// locally produced early attestations reference a tracked source block.
    ///
    /// # Errors
    /// - [`ForkchoiceError::AnchorStateRootMismatch`] when
    ///   `anchor_block.state_root != state.hash_tree_root()`. Both inputs
    ///   are dropped.
    /// - [`ForkchoiceError::AnchorTimeOverflow`] when
    ///   `anchor_block.slot * INTERVALS_PER_SLOT` overflows `u64`.
    pub fn from_anchor(state: State, anchor_block: Block) -> Result<Self, ForkchoiceError> {
        // `latest_justified` / `latest_finalized` are `Copy`; read them before
        // `state` moves into `seed_anchor`.
        let justified = state.latest_justified;
        let finalized = state.latest_finalized;
        Self::seed_anchor(state, anchor_block, |anchor_root, anchor_slot| {
            (
                normalize_genesis_checkpoint(justified, anchor_slot, anchor_root),
                normalize_genesis_checkpoint(finalized, anchor_slot, anchor_root),
            )
        })
    }

    /// Resume constructor: trusts `anchor_block` as this node's own persisted
    /// head and seeds BOTH `latest_justified` and `latest_finalized` to the
    /// anchor itself — `(anchor_root, anchor_block.slot)`.
    ///
    /// Unlike [`Self::from_anchor`], which copies the justified/finalized
    /// checkpoints out of `state`, this pins them to the anchor. That is
    /// required when re-anchoring a restarted single node at its persisted head:
    /// `from_anchor` would seed `latest_justified` from the state, whose root is
    /// an ancestor block absent from the anchor-only block map, so the first
    /// LMD-GHOST head walk ([`crate::helpers::get_fork_choice_head`], which
    /// requires its start root to be tracked) would fail with
    /// [`ForkchoiceError::UnknownRootBlock`]. Anchoring justified and finalized
    /// at the head keeps the walk's start root in the map.
    ///
    /// Semantics: the reported finalized slot becomes the head slot — always
    /// `>=` the pre-restart finalized slot, so finalization stays monotonic.
    /// This is the weak-subjectivity trusted-restart model appropriate for a
    /// standalone node that never reorgs its own chain. The store's
    /// `latest_finalized` (pinned here at the anchor) therefore diverges from
    /// the anchor `state.latest_finalized` (the true lower-slot ancestor) until
    /// a post-restart block finalizes at `slot > anchor.slot` and the
    /// checkpoint-adoption path re-adopts; the store field is never fed back
    /// into the state-transition function, so this is intentional, not an
    /// inconsistency.
    ///
    /// Scope: this over-reports finalization (it asserts `finalized == justified
    /// == head`), which is sound only for a standalone node that trusts its own
    /// persisted head. If this store is ever wired into multi-peer finalization
    /// exchange, the anchor-pinned `latest_finalized` MUST NOT be gossiped as a
    /// real FFG checkpoint — use a reconstruction that preserves the true
    /// finalized ancestor instead.
    ///
    /// # Errors
    /// Same as [`Self::from_anchor`].
    pub fn from_trusted_head(state: State, anchor_block: Block) -> Result<Self, ForkchoiceError> {
        Self::seed_anchor(state, anchor_block, |anchor_root, anchor_slot| {
            let cp = Checkpoint::new(anchor_root, anchor_slot);
            (cp, cp)
        })
    }

    /// Shared body of [`Self::from_anchor`] and [`Self::from_trusted_head`]:
    /// validates state-root parity and the anchor clock, then builds the
    /// single-anchor store with the caller-selected `(justified, finalized)`
    /// checkpoints (given the derived anchor root and slot).
    fn seed_anchor(
        state: State,
        anchor_block: Block,
        checkpoints: impl FnOnce(Bytes32, Slot) -> (Checkpoint, Checkpoint),
    ) -> Result<Self, ForkchoiceError> {
        // 1. State-root parity check (cheap; runs before any move).
        let want_state_root: Bytes32 = state.hash_tree_root().into();
        if anchor_block.state_root != want_state_root {
            return Err(ForkchoiceError::AnchorStateRootMismatch {
                got: anchor_block.state_root,
                want: want_state_root,
            });
        }

        // 2. Derived values.
        let anchor_root: Bytes32 = anchor_block.hash_tree_root().into();
        let time = anchor_block
            .slot
            .get()
            .checked_mul(INTERVALS_PER_SLOT)
            .map(Time::new)
            .ok_or(ForkchoiceError::AnchorTimeOverflow {
                slot: anchor_block.slot,
                intervals_per_slot: INTERVALS_PER_SLOT,
            })?;

        // 3. Build the store with the seeded fields; remaining fields
        //    (maps, block_order, vote maps) take their `Default` empty values.
        let (latest_justified, latest_finalized) = checkpoints(anchor_root, anchor_block.slot);
        let mut store = Self {
            time,
            config: state.config,
            head: anchor_root,
            safe_target: anchor_root,
            latest_justified,
            latest_finalized,
            ..Default::default()
        };
        store.insert_block(anchor_root, anchor_block, state);
        Ok(store)
    }

    /// Returns the store's current forkchoice time (intervals since genesis).
    pub fn time(&self) -> Time {
        self.time
    }

    /// Returns the in-state runtime parameters carried alongside the chain.
    #[must_use]
    pub fn config(&self) -> &ProtocolConfig {
        &self.config
    }

    /// Validator-set size derived from the head block's post-state registry.
    ///
    /// The spec reads the head state's registry length at every use site
    /// rather than caching a scalar, so a registry that changes at the head is
    /// picked up without an explicit refresh.
    ///
    /// # Errors
    /// [`ForkchoiceError::HeadStateNotFound`] when the head's post-state is
    /// not tracked. Callers MUST propagate rather than substitute a default:
    /// on the ingress path this count is the bound that keeps a peer from
    /// flooding the vote pool with arbitrary validator ids, so an unknown
    /// count has to fail closed.
    ///
    /// Only a corrupt store can reach the error: every real constructor
    /// ([`Self::from_anchor`], [`Self::from_trusted_head`]) inserts the anchor
    /// post-state under the head root. [`Store::default()`] does not, and is
    /// documented as not a usable fork-choice state.
    fn head_validator_count(&self) -> Result<u64, ForkchoiceError> {
        let state = self
            .states
            .get(&self.head)
            .ok_or(ForkchoiceError::HeadStateNotFound { root: self.head })?;
        Ok(state.num_validators())
    }

    /// Returns the current canonical head root.
    #[must_use]
    pub fn head(&self) -> Bytes32 {
        self.head
    }

    /// Returns the current safe attestation target root.
    #[must_use]
    pub fn safe_target(&self) -> Bytes32 {
        self.safe_target
    }

    /// Returns the highest justified checkpoint known to the store.
    #[must_use]
    pub fn latest_justified(&self) -> Checkpoint {
        self.latest_justified
    }

    /// Returns the highest finalized checkpoint known to the store.
    #[must_use]
    pub fn latest_finalized(&self) -> Checkpoint {
        self.latest_finalized
    }

    /// Returns the block-insertion order (first-seen order, no duplicates).
    #[must_use]
    pub fn block_order(&self) -> &[Bytes32] {
        &self.block_order
    }

    /// Resolves a tracked block by root. Returns `None` if the root is
    /// unknown to the store.
    #[must_use]
    pub fn block(&self, root: &Bytes32) -> Option<&Block> {
        self.blocks.get(root)
    }

    /// Resolves a tracked post-state by block root. Returns `None` if the
    /// root is unknown to the store.
    ///
    /// Returns the `Arc`-wrapped state: `.clone()` on the result is a refcount
    /// bump, not a deep copy. Callers needing an owned, mutable `State` (e.g.
    /// to run a state transition) deref-clone via `(**arc).clone()`.
    #[must_use]
    pub fn state(&self, root: &Bytes32) -> Option<&Arc<State>> {
        self.states.get(root)
    }

    /// Reports whether `root` is known to the store.
    #[must_use]
    pub fn has_block(&self, root: &Bytes32) -> bool {
        self.blocks.contains_key(root)
    }

    /// Returns the accepted full [`SignedAttestation`]s by validator. Populated
    /// by [`Self::process_attestation`] (on-chain branch) and promoted
    /// from [`Self::latest_new_votes`] by [`Self::accept_new_votes`].
    #[must_use]
    pub fn latest_known_votes(&self) -> &HashMap<ValidatorIndex, SignedAttestation> {
        &self.latest_known_votes
    }

    /// Returns pending full [`SignedAttestation`]s received via gossip but not
    /// yet promoted into [`Self::latest_known_votes`].
    #[must_use]
    pub fn latest_new_votes(&self) -> &HashMap<ValidatorIndex, SignedAttestation> {
        &self.latest_new_votes
    }

    /// Internal: invariant-preserving block-insert.
    ///
    /// On the first insert of `root`, both maps and `block_order` gain the
    /// triple. On any re-insert the call is a no-op (no duplicate in
    /// `block_order`). The wider validating wrapper is
    /// [`Self::track_block`]; this method exists so anchor seeding,
    /// `track_block`, and the `block_order` invariant test share one path.
    pub(crate) fn insert_block(&mut self, root: Bytes32, block: Block, state: State) {
        if self.blocks.insert(root, block).is_none() {
            self.states.insert(root, Arc::new(state));
            self.block_order.push(root);
        }
    }

    // ==================================================================
    // tick_interval — 4-phase clock advance
    // ==================================================================

    /// Slot index derived from the current clock value.
    #[must_use]
    pub fn current_slot(&self) -> u64 {
        self.time.slot()
    }

    /// Intra-slot interval index derived from the current clock value.
    #[must_use]
    pub fn current_interval(&self) -> u64 {
        self.time.interval()
    }

    /// Spec [`Phase`] of the current clock value.
    pub fn current_phase(&self) -> Phase {
        self.time.phase()
    }

    /// Advances the forkchoice clock by one interval and dispatches the
    /// phase-specific hook based on the *new* time.
    ///
    /// The four phases are mandated by leanSpec; see [`Phase`] for the
    /// variant-to-hook mapping. Exhaustive matching on [`Phase`] guards
    /// future spec changes against silent reordering.
    ///
    /// # Errors
    /// - [`ForkchoiceError::TimeOverflow`] when `self.time().get() == u64::MAX`.
    /// - Forwarded from [`Self::accept_new_votes`] /
    ///   [`Self::update_safe_target`].
    pub fn tick_interval(&mut self, has_proposal: bool) -> Result<(), ForkchoiceError> {
        let next = self
            .time
            .checked_advance()
            .ok_or(ForkchoiceError::TimeOverflow { time: self.time })?;
        self.time = next;

        match next.phase() {
            Phase::Proposal if has_proposal => self.accept_new_votes(),
            Phase::Proposal | Phase::Idle => Ok(()),
            Phase::UpdateSafeTarget => self.update_safe_target(),
            Phase::AcceptNewVotes => self.accept_new_votes(),
        }
    }

    // ==================================================================
    // Attestation processing
    // ==================================================================

    /// Validates the structural and timing rules an [`AttestationData`] must
    /// satisfy before [`Self::process_attestation`] will route its envelope into
    /// either vote pool.
    ///
    /// Mirrors leanSpec `forkchoice/store.py::Store.validate_attestation`
    /// (`:277-:325 @ 0c9528ac`) predicate for predicate, in the reference's four
    /// groups: availability, topology, consistency, time. Like the reference it
    /// takes DATA rather than a signed envelope — the predicates are
    /// signature-agnostic. This client's extra validator-index bound is NOT here;
    /// it lives in [`Self::validate_validator_index`], which
    /// [`Self::process_attestation`] runs first.
    ///
    /// One divergence: the source root is RESOLVED before lookup. The reference
    /// normalizes nothing on ingress (`store.py:299`, `:313`) because its producer
    /// substitutes the genesis root before signing (`store.py:1289-:1295`). This
    /// client tolerates a peer that does not, for the lookup only — see
    /// [`Self::resolved_source_root`]. The head checkpoint gets NO such
    /// tolerance: the reference builds it from `self.head`
    /// (`store.py:1299-:1302`), so a well-formed vote never carries a placeholder
    /// head, and widening the guard would buy no interop case.
    ///
    /// That resolution WIDENS this function's acceptance surface, which matters
    /// because it is `pub`: a caller passing a genesis-placeholder vote now gets
    /// `Ok(())` where it previously got [`ForkchoiceError::UnknownSourceBlock`].
    /// The rejection used to happen here only because the caller had already
    /// rewritten the vote; the resolution moved inward when that rewrite was
    /// removed.
    ///
    /// # Errors
    /// - [`ForkchoiceError::UnknownSourceBlock`] when the vote's RESOLVED source
    ///   root is not tracked by the store. Resolved, not declared: a slot-0
    ///   genesis-placeholder vote declares an all-zero root, which is never tracked,
    ///   and still returns `Ok` — see [`Self::resolved_source_root`]. The error
    ///   nonetheless always REPORTS the declared root, because substitution only
    ///   happens when the justified root is already tracked, so a substituted root
    ///   cannot reach this error.
    /// - [`ForkchoiceError::UnknownTargetBlock`] when the target checkpoint root
    ///   is not tracked by the store.
    /// - [`ForkchoiceError::UnknownAttestationHeadBlock`] when the head
    ///   checkpoint root is not tracked by the store.
    /// - [`ForkchoiceError::SourceSlotExceedsTarget`] when either the
    ///   resolved source block's slot exceeds the resolved target block's
    ///   slot or `vote.source.slot > vote.target.slot`.
    /// - [`ForkchoiceError::HeadSlotBelowTarget`] when
    ///   `vote.head.slot < vote.target.slot`.
    /// - [`ForkchoiceError::SourceCheckpointSlotMismatch`] /
    ///   [`ForkchoiceError::TargetCheckpointSlotMismatch`] /
    ///   [`ForkchoiceError::HeadCheckpointSlotMismatch`] when a declared
    ///   checkpoint slot disagrees with its resolved block's slot.
    /// - [`ForkchoiceError::AttestationFutureLimitOverflow`] when
    ///   `current_vote_slot() + 1` would overflow `u64`.
    /// - [`ForkchoiceError::AttestationTooFarInFuture`] when
    ///   `vote.slot > current_vote_slot() + 1`.
    pub fn validate_attestation(&self, vote: &AttestationData) -> Result<(), ForkchoiceError> {
        // Availability — a vote cannot be counted for blocks this store has not
        // seen (`store.py:299-:301`).
        let source_block = self.lookup_block(self.resolved_source_root(vote), |root| {
            ForkchoiceError::UnknownSourceBlock { root }
        })?;
        let target_block = self.lookup_block(vote.target.root, |root| {
            ForkchoiceError::UnknownTargetBlock { root }
        })?;
        let head_block = self.lookup_block(vote.head.root, |root| {
            ForkchoiceError::UnknownAttestationHeadBlock { root }
        })?;

        // Topology — history is linear and monotonic: source <= target <= head,
        // so `head >= source` follows by transitivity (`store.py:307-:308`).
        if source_block.slot > target_block.slot || vote.source.slot > vote.target.slot {
            return Err(ForkchoiceError::SourceSlotExceedsTarget);
        }
        if vote.head.slot < vote.target.slot {
            return Err(ForkchoiceError::HeadSlotBelowTarget {
                head_slot: vote.head.slot,
                target_slot: vote.target.slot,
            });
        }

        // Consistency — a declared checkpoint slot must be the block's own slot
        // (`store.py:313-:318`). One comparison written once, applied to the
        // three checkpoints in the reference's order. This stays INSIDE the
        // consistency group, so it does not interleave the spec's groups the way
        // a per-checkpoint resolve-and-check helper would.
        let consistency = [
            (
                source_block.slot,
                vote.source.slot,
                ForkchoiceError::SourceCheckpointSlotMismatch,
            ),
            (
                target_block.slot,
                vote.target.slot,
                ForkchoiceError::TargetCheckpointSlotMismatch,
            ),
            (
                head_block.slot,
                vote.head.slot,
                ForkchoiceError::HeadCheckpointSlotMismatch,
            ),
        ];
        for (block_slot, declared_slot, mismatch) in consistency {
            if block_slot != declared_slot {
                return Err(mismatch);
            }
        }

        // Time — one slot of clock disparity is tolerated, no further
        // (`store.py:324-:325`).
        let current = self.current_vote_slot();
        let limit = current
            .advance()
            .ok_or(ForkchoiceError::AttestationFutureLimitOverflow {
                current_slot: current,
            })?;
        if vote.slot > limit {
            return Err(ForkchoiceError::AttestationTooFarInFuture {
                vote_slot: vote.slot,
                limit,
            });
        }
        Ok(())
    }

    /// Bounds a vote's `validator_id` against the head post-state's registry.
    ///
    /// NOT a leanSpec predicate: the reference binds its own index check to
    /// signature verification against the target block's state
    /// (`forkchoice/store.py:370-:372 @ 0c9528ac`), and
    /// [`Self::validate_attestation`] stays signature-agnostic. This guard
    /// exists because the vote pools are keyed by validator id — without it a
    /// peer can flood them with arbitrary `u64` ids at roughly 3.2 KiB per
    /// entry. It therefore runs FIRST, ahead of any block lookup, and fails
    /// closed when the head post-state is unknown.
    ///
    /// # Errors
    /// - [`ForkchoiceError::HeadStateNotFound`] forwarded from
    ///   [`Self::head_validator_count`] when the head's post-state is untracked.
    /// - [`ForkchoiceError::ValidatorIndexOutOfRange`] when the id is at or past
    ///   the head registry length.
    pub(crate) fn validate_validator_index(
        &self,
        validator_id: ValidatorIndex,
    ) -> Result<(), ForkchoiceError> {
        let num_validators = self.head_validator_count()?;
        let vid = validator_id.get();
        if vid >= num_validators {
            return Err(ForkchoiceError::ValidatorIndexOutOfRange {
                validator_id: vid,
                num_validators,
            });
        }
        Ok(())
    }

    /// Applies a [`SignedAttestation`] either as an on-chain vote
    /// (`is_from_block == true`) or a gossip vote (`is_from_block ==
    /// false`).
    ///
    /// Returns `true` when the call mutated either vote pool, `false`
    /// otherwise. Re-applying the same vote at the same `vote.slot` is a
    /// no-op (idempotency).
    ///
    /// The envelope is stored VERBATIM. `source.root` is inside the signed
    /// preimage (`hash_tree_root(attestation)`), so a vote must reach the pool
    /// byte-identical to the one that arrived or its signature verifies against
    /// nothing. A genesis-placeholder source is resolved for VALIDATION only —
    /// see [`Self::resolved_source_root`].
    ///
    /// Verbatim storage is NECESSARY for a pooled signature to be meaningful, not
    /// SUFFICIENT for it to be trustworthy. Nothing on either ingress path verifies
    /// it: the gossip caller runs no verifier at all, and the on-chain caller
    /// SYNTHESIZES the envelope by pairing a body attestation with a positional
    /// signature that no block root commits to, falling back to a default signature
    /// when the list is short. So a pooled signature is peer-chosen or absent, and
    /// whichever change starts CONSUMING it — publishing it in a produced block, or
    /// forwarding it — MUST verify it rather than trusting that it arrived intact.
    /// This function guarantees only that it was not corrupted after arrival.
    ///
    /// On-chain branch:
    /// 1. Insert into `latest_known_votes` only when strictly newer.
    /// 2. Evict from `latest_new_votes` only when the pending entry is
    ///    strictly older. Eviction compares `vote.message.data.slot` (the
    ///    attestation slot), not `vote.message.data.target.slot`.
    ///
    /// Gossip branch: insert into `latest_new_votes` when strictly newer.
    /// Future-slot votes within the `current_vote_slot + 1` window are
    /// admitted; they can only matter once promoted by
    /// [`Self::accept_new_votes`].
    ///
    /// # Errors
    /// Forwards any error from [`Self::validate_validator_index`], which runs
    /// FIRST so a forged validator id is rejected before any block lookup, then
    /// from [`Self::validate_attestation`].
    pub fn process_attestation(
        &mut self,
        signed_vote: SignedAttestation,
        is_from_block: bool,
    ) -> Result<bool, ForkchoiceError> {
        self.validate_validator_index(signed_vote.message.validator_id)?;
        self.validate_attestation(&signed_vote.message.data)?;

        let validator = signed_vote.message.validator_id;
        let vote_slot = signed_vote.message.data.slot;

        if is_from_block {
            let promoted = insert_if_newer(&mut self.latest_known_votes, validator, signed_vote);
            let evicted = evict_if_older(&mut self.latest_new_votes, validator, vote_slot);
            return Ok(promoted || evicted);
        }
        // Gossip branch carries no freshness gate against the current vote
        // slot — `validate_attestation` already capped `vote.slot` at
        // `current_vote_slot + 1`.
        Ok(insert_if_newer(
            &mut self.latest_new_votes,
            validator,
            signed_vote,
        ))
    }

    /// Slot derived from the store's current clock value. Equivalent to
    /// `Slot::new(self.time.slot())` but kept as a single named helper so
    /// future spec edits to the slot derivation land in one place.
    #[must_use]
    pub(crate) const fn current_vote_slot(&self) -> Slot {
        Slot::new(self.time.slot())
    }

    /// Resolves the block root a vote's `source` checkpoint refers to, WITHOUT
    /// editing the vote.
    ///
    /// Some peers emit a slot-0 vote whose source checkpoint is still the all-zero
    /// placeholder, because their producer had not yet substituted the real genesis
    /// root. This store keeps the slot-0 justified checkpoint at the anchor root
    /// ([`Self::from_anchor`]), so that placeholder means "the anchor" here, and
    /// resolving it keeps such a vote admissible.
    ///
    /// The resolution is READ-ONLY by necessity, not by preference. A vote's
    /// signature is made over `hash_tree_root(attestation)` and `source.root` is
    /// inside that preimage, so writing a resolved root back into the vote would
    /// leave a stored envelope whose signature verifies against nothing — and, once
    /// the producer assembles a full positional signature list, would publish that
    /// envelope in a block. The reference implementation substitutes on the
    /// PRODUCER side (`forkchoice/store.py:1289-:1295 @ 0c9528ac`), before anything
    /// is signed, and never edits a payload on ingress
    /// (`store.py:363`, `:388-:390`).
    ///
    /// The guard stays narrow: only an all-zero source on a SLOT-0 vote against a
    /// slot-0 anchor this store has actually tracked resolves. Every other zero root
    /// falls through and fails the normal source lookup.
    ///
    /// The `vote.slot.is_zero()` clause is load-bearing, not decoration. Without it
    /// the tolerance extends to a vote at any slot up to `current_vote_slot() + 1`,
    /// and [`insert_if_newer`] is first-wins at equal slot — so a placeholder vote
    /// parked at the future cap would refuse every honest vote from that validator
    /// at or below it. Such a vote is also excluded from produced block bodies (its
    /// verbatim source cannot match the candidate's justified checkpoint), so it
    /// would carry head weight on gossip-connected peers and none on blocks-only
    /// peers: a head split. Pinning the clause to slot 0 keeps the tolerance to the
    /// shape it was introduced for — a peer's slot-0 bootstrap vote — where the
    /// vote can only ever be superseded upward and the divergence cannot arise.
    fn resolved_source_root(&self, vote: &AttestationData) -> Bytes32 {
        let is_genesis_placeholder = vote.source == Checkpoint::default()
            && vote.slot.is_zero()
            && vote.target.slot.is_zero()
            && vote.target.root == self.latest_justified.root
            && self.latest_justified.slot.is_zero()
            && self
                .blocks
                .get(&self.latest_justified.root)
                .is_some_and(|block| block.slot.is_zero());

        if is_genesis_placeholder {
            self.latest_justified.root
        } else {
            vote.source.root
        }
    }

    /// Resolves a tracked block by root, raising the caller-supplied error
    /// variant when the root is unknown. Keeps the two checkpoint lookups
    /// in [`Self::validate_attestation`] DRY without committing to a
    /// single error variant.
    fn lookup_block(
        &self,
        root: Bytes32,
        err: impl FnOnce(Bytes32) -> ForkchoiceError,
    ) -> Result<&Block, ForkchoiceError> {
        self.blocks.get(&root).ok_or_else(|| err(root))
    }

    // ==================================================================
    // Phase hook bodies
    // ==================================================================

    /// Promotes pending votes into the known vote set and refreshes the
    /// forkchoice head.
    ///
    /// Exposed so the engine layer can refresh the canonical head after a
    /// successful block import without re-driving the proposal-head flow.
    ///
    /// # Errors
    /// Forwards [`ForkchoiceError`] variants raised by the internal head
    /// refresh (currently any error from
    /// [`crate::helpers::get_fork_choice_head`]).
    pub fn accept_new_votes(&mut self) -> Result<(), ForkchoiceError> {
        let promoted = std::mem::take(&mut self.latest_new_votes);
        self.latest_known_votes.extend(promoted);
        self.update_head()
    }

    /// Recomputes the safe attestation target. Scoring is gated by the
    /// `ceil(2N/3)` supermajority threshold and walks the vote's *head*
    /// checkpoint (not the FFG target), matching the canonical LMD-GHOST
    /// head selection.
    ///
    /// # Errors
    /// Forwards [`ForkchoiceError`] variants raised by
    /// [`crate::helpers::get_fork_choice_head`].
    pub(crate) fn update_safe_target(&mut self) -> Result<(), ForkchoiceError> {
        // Supermajority threshold: `ceil(2N/3)` — matches leanSpec's
        // `-(-N*2 // 3)`. Spelled with `u64::div_ceil` for overflow safety.
        let min_target_score = self.head_validator_count()?.saturating_mul(2).div_ceil(3);
        let checkpoints = vote_head_checkpoints(&self.latest_new_votes);
        self.safe_target = get_fork_choice_head(
            &self.blocks,
            self.latest_justified.root,
            &checkpoints,
            min_target_score,
        )?;
        Ok(())
    }

    /// Refreshes the canonical head from `latest_known_votes` using the
    /// `min_score = 0` LMD-GHOST walk.
    ///
    /// The `latest_finalized` carry-snapshot (the OLD-head's
    /// `state.latest_finalized` written AFTER the head switch) is part of
    /// the block-import flow, not this hook.
    fn update_head(&mut self) -> Result<(), ForkchoiceError> {
        let checkpoints = vote_head_checkpoints(&self.latest_known_votes);
        self.head =
            get_fork_choice_head(&self.blocks, self.latest_justified.root, &checkpoints, 0)?;
        Ok(())
    }

    // ==================================================================
    // Block tracking + proposal/vote-target accessors (used by production)
    // ==================================================================

    /// Stores a `(block, post_state)` pair after validating
    /// `block.state_root` against `hash_tree_root(post_state)` and
    /// confirming the parent root is tracked. Newly learned justified /
    /// finalized checkpoints from the post-state are adopted when their
    /// roots are already tracked by the store.
    ///
    /// Returns `true` when the pair was newly tracked, `false` when the
    /// block root is already present (idempotent). The store performs no
    /// vote processing or head refresh — this is the non-import sibling
    /// of the full block-import flow.
    ///
    /// # Errors
    /// - [`ForkchoiceError::BlockStateRootMismatch`] when `block.state_root`
    ///   disagrees with `hash_tree_root(post_state)`.
    /// - [`ForkchoiceError::ParentBlockNotFound`] when `block.parent_root`
    ///   is non-zero and not tracked by the store.
    pub fn track_block(
        &mut self,
        block: Block,
        post_state: State,
    ) -> Result<bool, ForkchoiceError> {
        let want_state_root: Bytes32 = post_state.hash_tree_root().into();
        if block.state_root != want_state_root {
            return Err(ForkchoiceError::BlockStateRootMismatch {
                got: block.state_root,
                want: want_state_root,
            });
        }
        let root: Bytes32 = block.hash_tree_root().into();
        if self.blocks.contains_key(&root) {
            return Ok(false);
        }
        if block.parent_root != Bytes32::zero() && !self.blocks.contains_key(&block.parent_root) {
            return Err(ForkchoiceError::ParentBlockNotFound {
                root: block.parent_root,
            });
        }
        let latest_justified = post_state.latest_justified;
        let latest_finalized = post_state.latest_finalized;
        self.insert_block(root, block, post_state);
        self.adopt_post_state_checkpoints(latest_justified, latest_finalized);
        Ok(true)
    }

    /// Returns the current proposal head, after promoting any pending
    /// votes into the known vote set.
    ///
    /// Per the leanSpec proposal-hook contract, tick state is driven
    /// exclusively by the regular tick loop — the proposal call doesn't
    /// advance the clock, so it doesn't need a slot argument. Future spec
    /// edits that re-introduce slot-awareness can reinstate the parameter.
    ///
    /// # Errors
    /// Forwards [`ForkchoiceError`] variants raised by
    /// [`Self::accept_new_votes`].
    pub fn get_proposal_head(&mut self) -> Result<Bytes32, ForkchoiceError> {
        self.accept_new_votes()?;
        Ok(self.head)
    }

    /// Selects this node's attestation target: walk back from the head toward
    /// the safe target, then back further until the slot is justifiable.
    ///
    /// Mirrors leanSpec `forkchoice/store.py::Store.get_attestation_target`
    /// (`:1202-:1259 @ 0c9528ac`), whose two walks are reproduced in order: at
    /// most [`JUSTIFICATION_LOOKBACK_SLOTS`] steps toward the safe target
    /// (`:1241-:1245`), then an unbounded walk until
    /// [`Slot::is_justifiable_after`] holds against the finalized slot
    /// (`:1251-:1254`).
    ///
    /// One divergence, deliberate: both walks are floored at the finalized
    /// checkpoint. The reference's second loop is unbounded because
    /// `Slot.is_justifiable_after` ASSERTS the candidate is at or after the
    /// finalized slot (`containers/slot.py:50`); this client's port returns
    /// `false` instead, which would turn the same input into a walk off the
    /// bottom of the chain. The input is reachable:
    /// [`Self::adopt_post_state_checkpoints`] advances `latest_finalized`
    /// without refreshing `safe_target`, so walk 1 can land below the finalized
    /// slot. The floor makes termination a property of this function — a parent
    /// step strictly decreases the slot, so the walk reaches the floor from any
    /// starting point — rather than of the caller's store hygiene. The bound is
    /// a SLOT comparison, not root-equality: the finalized block need not lie on
    /// the cursor's ancestry.
    ///
    /// The floor bounds TERMINATION, not the post-condition, and the two cases
    /// differ:
    ///
    /// - The finalized block IS on the head's ancestry. Then the walk stops
    ///   exactly at it — distance 0 is justifiable — and every returned slot is
    ///   justifiable after the finalized slot.
    /// - It is NOT. [`Self::update_head`] descends from `latest_justified` while
    ///   [`Self::adopt_post_state_checkpoints`] takes `latest_finalized` from any
    ///   tracked post-state, so the two can sit on different branches. A single
    ///   parent step can then skip from above the finalized slot to below it, and
    ///   both loops exit on the floor with a target that is NOT justifiable.
    ///
    /// That undershoot is left visible rather than papered over. Substituting
    /// `self.latest_finalized` would be worse — that checkpoint is on the other
    /// branch, so the node would attest off its own head's ancestry — and no
    /// choice of target is correct on a store whose head and finalized checkpoint
    /// have diverged. The behaviour is pinned by
    /// `target_selection_undershoots_when_finalized_is_off_ancestry` so it stays
    /// deliberate, and the underlying divergence is tracked separately.
    ///
    /// # Errors
    /// - [`ForkchoiceError::UnknownHeadBlock`] when `self.head` is not in
    ///   the block map.
    /// - [`ForkchoiceError::UnknownSafeTarget`] when `self.safe_target` is
    ///   not in the block map.
    /// - [`ForkchoiceError::ParentBlockNotFound`] when either walk steps past
    ///   a block whose parent is absent from the block map.
    pub fn get_vote_target(&self) -> Result<Checkpoint, ForkchoiceError> {
        let mut cursor = self.head;
        let mut cursor_block =
            self.lookup_block(cursor, |root| ForkchoiceError::UnknownHeadBlock { root })?;
        let safe_slot = self
            .lookup_block(self.safe_target, |root| {
                ForkchoiceError::UnknownSafeTarget { root }
            })?
            .slot;
        let finalized_slot = self.latest_finalized.slot;
        let floor_slot = safe_slot.max(finalized_slot);

        // Walk 1 — toward the safe target, bounded by the lookback window.
        for _ in 0..JUSTIFICATION_LOOKBACK_SLOTS {
            if cursor_block.slot <= floor_slot {
                break;
            }
            (cursor, cursor_block) = self.parent_of(cursor_block)?;
        }

        // Walk 2 — back until the slot can be justified. The loop cannot run
        // past the floor: either the cursor reaches `finalized_slot` (distance
        // zero, justifiable) or a step lands below it, and both exits are
        // guarded. The second case is the documented undershoot above.
        while cursor_block.slot > finalized_slot
            && !cursor_block.slot.is_justifiable_after(finalized_slot)
        {
            (cursor, cursor_block) = self.parent_of(cursor_block)?;
        }

        // A cursor below `finalized_slot` here means the finalized checkpoint is
        // off the head's ancestry — documented above, pinned by
        // `target_selection_undershoots_when_finalized_is_off_ancestry`. NOT an
        // assertion: the condition is peer-reachable, no target is correct on a
        // diverged store, and this function must still return one.
        Ok(Checkpoint::new(cursor, cursor_block.slot))
    }

    /// Resolves a block's parent, returning the `(root, block)` pair. Shared by
    /// both walks in [`Self::get_vote_target`] so the step and its error variant
    /// are written once.
    fn parent_of(&self, block: &Block) -> Result<(Bytes32, &Block), ForkchoiceError> {
        let parent_root = block.parent_root;
        let parent_block = self.lookup_block(parent_root, |root| {
            ForkchoiceError::ParentBlockNotFound { root }
        })?;
        Ok((parent_root, parent_block))
    }

    /// Test-only builder that overrides the constructor-seeded time.
    /// Lets table-driven and property tests position the clock without
    /// running `tick_interval` from genesis.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_time_for_test(mut self, time: Time) -> Self {
        self.time = time;
        self
    }

    /// Test-only direct pending-vote insertion. Used by `update_head` and
    /// `update_safe_target` tests that need to seed votes without driving
    /// the full `process_attestation` validation path.
    #[cfg(test)]
    pub(crate) fn insert_new_vote_for_test(&mut self, vote: SignedAttestation) {
        self.latest_new_votes
            .insert(vote.message.validator_id, vote);
    }

    /// Test-only setter for the justified-checkpoint descent origin.
    /// Production code mutates `latest_justified` only by adopting
    /// checkpoints from tracked post-states.
    #[cfg(test)]
    pub(crate) fn set_latest_justified_for_test(&mut self, checkpoint: Checkpoint) {
        self.latest_justified = checkpoint;
    }

    /// Adopts newer checkpoints observed in a tracked block's post-state.
    ///
    /// State transition is the source of truth for justification/finality;
    /// forkchoice uses these cached checkpoints as the LMD-GHOST descent
    /// origins for head, safe-target, and attestation production. Ignore
    /// checkpoints whose roots are not tracked so synthetic/default states
    /// cannot move the store to an unresolvable origin.
    fn adopt_post_state_checkpoints(
        &mut self,
        latest_justified: Checkpoint,
        latest_finalized: Checkpoint,
    ) {
        if self.should_adopt_checkpoint(latest_justified, self.latest_justified) {
            self.latest_justified = latest_justified;
        }
        if self.should_adopt_checkpoint(latest_finalized, self.latest_finalized) {
            self.latest_finalized = latest_finalized;
        }
    }

    fn should_adopt_checkpoint(&self, candidate: Checkpoint, current: Checkpoint) -> bool {
        candidate.slot > current.slot && self.blocks.contains_key(&candidate.root)
    }
}

/// Inserts `vote` only when the map's existing entry for `validator` is
/// strictly older (by `message.slot`). Returns `true` when the map was
/// mutated.
fn insert_if_newer(
    map: &mut HashMap<ValidatorIndex, SignedAttestation>,
    validator: ValidatorIndex,
    vote: SignedAttestation,
) -> bool {
    match map.get(&validator) {
        Some(existing) if existing.message.data.slot >= vote.message.data.slot => false,
        _ => {
            map.insert(validator, vote);
            true
        }
    }
}

/// Removes the map's entry for `validator` when its `message.slot` is
/// strictly older than `newer_than`. Returns `true` when the map was
/// mutated.
fn evict_if_older(
    map: &mut HashMap<ValidatorIndex, SignedAttestation>,
    validator: ValidatorIndex,
    newer_than: Slot,
) -> bool {
    match map.entry(validator) {
        Entry::Occupied(e) if e.get().message.data.slot < newer_than => {
            e.remove();
            true
        }
        _ => false,
    }
}

/// Extracts the per-validator `head` checkpoint from a vote map. Shared by
/// the head-refresh and safe-target hooks; both score by the LMD-GHOST
/// *head* checkpoint, not the FFG target.
fn vote_head_checkpoints(
    votes: &HashMap<ValidatorIndex, SignedAttestation>,
) -> HashMap<ValidatorIndex, Checkpoint> {
    votes
        .iter()
        .map(|(v, sv)| (*v, sv.message.data.head))
        .collect()
}

fn normalize_genesis_checkpoint(
    checkpoint: Checkpoint,
    anchor_slot: Slot,
    anchor_root: Bytes32,
) -> Checkpoint {
    if anchor_slot.is_zero() && checkpoint == Checkpoint::default() {
        Checkpoint::new(anchor_root, Slot::ZERO)
    } else {
        checkpoint
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::Slot;
    use static_assertions::assert_impl_all;
    use std::hash::Hash;
    use types::Bytes32;

    use crate::test_fixtures::{anchor_pair, anchor_pair_at_slot};

    // -- Compile-time witness ----------------------------------------------

    assert_impl_all!(Bytes32: Hash, Eq, PartialEq);

    // -- Validation: rejection paths ---------------------------------------

    #[test]
    fn from_anchor_rejects_state_root_mismatch() {
        let (state, mut block) = anchor_pair(4);
        let want: Bytes32 = state.hash_tree_root().into();
        block.state_root = Bytes32::new([0xff; 32]);
        let err = Store::from_anchor(state, block).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::AnchorStateRootMismatch {
                got: Bytes32::new([0xff; 32]),
                want,
            }
        );
    }

    #[test]
    fn from_anchor_rejects_when_slot_intervals_overflow_u64() {
        let overflow_slot = Slot::new(u64::MAX / INTERVALS_PER_SLOT + 1);
        let (state, block) = anchor_pair_at_slot(overflow_slot, 4);
        let err = Store::from_anchor(state, block).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::AnchorTimeOverflow {
                slot: overflow_slot,
                intervals_per_slot: INTERVALS_PER_SLOT,
            }
        );
    }

    // -- Construction: head + safe target + clock --------------------------

    #[test]
    fn from_anchor_seeds_head_and_safe_target_to_anchor_root() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.head(), anchor_root);
        assert_eq!(store.safe_target(), anchor_root);
    }

    #[test]
    fn from_anchor_seeds_time_at_slot_zero() {
        let (state, block) = anchor_pair(4);
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.time(), Time::ZERO);
    }

    // -- Resume: from_trusted_head fixes the non-genesis head walk ----------

    #[test]
    fn from_trusted_head_resumes_where_from_anchor_would_break() {
        // Real, non-zero genesis block root — the root we make justified point
        // at, and which is NOT inserted into the slot-1 single-anchor map. Note
        // it must be non-zero: get_fork_choice_head treats a zero start root as
        // the min-slot-block sentinel, so a zero justified root would NOT
        // reproduce the bug.
        let (_g_state, g_block) = anchor_pair(4);
        let g_root: Bytes32 = g_block.hash_tree_root().into();
        assert_ne!(g_root, Bytes32::zero());

        // Slot-1 anchor pair; pin justified at the absent genesis root and
        // recompute the block's state_root AFTER the mutation so from_anchor's
        // parity check still holds.
        let (mut state1, mut block1) = anchor_pair_at_slot(Slot::new(1), 4);
        state1.latest_justified = Checkpoint::new(g_root, Slot::ZERO);
        block1.state_root = state1.hash_tree_root().into();
        let anchor_root: Bytes32 = block1.hash_tree_root().into();
        assert_ne!(state1.latest_justified.root, anchor_root);

        // from_anchor seeds justified at the absent, non-zero genesis root, so
        // the head walk cannot resolve its start root.
        let mut broken = Store::from_anchor(state1.clone(), block1.clone()).unwrap();
        assert!(
            matches!(
                broken.accept_new_votes(),
                Err(ForkchoiceError::UnknownRootBlock { .. })
            ),
            "from_anchor at a non-genesis head must fail the head walk on the absent justified root",
        );

        // from_trusted_head seeds justified + finalized at the anchor, so the
        // head walk starts from a tracked root and resolves.
        let mut resumed = Store::from_trusted_head(state1, block1).unwrap();
        assert_eq!(resumed.latest_justified().root, anchor_root);
        assert_eq!(resumed.latest_finalized().root, anchor_root);
        resumed
            .accept_new_votes()
            .expect("resumed head walk resolves from the anchor root");
        assert_eq!(resumed.head(), anchor_root);
    }

    #[test]
    fn from_anchor_seeds_time_at_non_zero_slot() {
        let (state, block) = anchor_pair_at_slot(Slot::new(5), 4);
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.time(), Time::new(5 * INTERVALS_PER_SLOT));
    }

    // -- Construction: maps + block_order ----------------------------------

    #[test]
    fn from_anchor_seeds_block_and_state_maps() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let pre_state_root: Bytes32 = state.hash_tree_root().into();
        let store = Store::from_anchor(state, block.clone()).unwrap();
        assert_eq!(store.block(&anchor_root), Some(&block));
        let stored_state_root: Bytes32 = store.state(&anchor_root).unwrap().hash_tree_root().into();
        assert_eq!(stored_state_root, pre_state_root);
    }

    #[test]
    fn from_anchor_seeds_block_order_singleton() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.block_order(), &[anchor_root]);
        assert!(store.has_block(&anchor_root));
    }

    // -- Construction: checkpoint inheritance ------------------------------

    #[test]
    fn from_anchor_normalizes_default_genesis_checkpoints_to_anchor() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let store = Store::from_anchor(state, block).unwrap();
        let anchor_checkpoint = Checkpoint::new(anchor_root, Slot::ZERO);
        assert_eq!(store.latest_justified(), anchor_checkpoint);
        assert_eq!(store.latest_finalized(), anchor_checkpoint);
    }

    #[test]
    fn from_anchor_inherits_non_default_justified_finalized_from_state() {
        let (mut state, mut block) = anchor_pair(4);
        let want_justified = Checkpoint::new(Bytes32::new([0x44; 32]), Slot::new(2));
        let want_finalized = Checkpoint::new(Bytes32::new([0x55; 32]), Slot::ONE);
        state.latest_justified = want_justified;
        state.latest_finalized = want_finalized;
        block.state_root = state.hash_tree_root().into();
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.latest_justified(), want_justified);
        assert_eq!(store.latest_finalized(), want_finalized);
    }

    #[test]
    fn from_anchor_empty_vote_maps() {
        let (state, block) = anchor_pair(4);
        let store = Store::from_anchor(state, block).unwrap();
        assert!(store.latest_known_votes().is_empty());
        assert!(store.latest_new_votes().is_empty());
    }

    // -- Default sentinel --------------------------------------------------

    #[test]
    fn default_is_structural_placeholder() {
        let store = Store::default();
        assert_eq!(store.time(), Time::ZERO);
        assert_eq!(store.head(), Bytes32::zero());
        assert_eq!(store.safe_target(), Bytes32::zero());
        assert!(store.block_order().is_empty());
        assert!(store.latest_known_votes().is_empty());
        assert!(store.latest_new_votes().is_empty());
        // Zero root is not tracked — accessors must return None.
        assert!(store.block(&Bytes32::zero()).is_none());
        assert!(store.state(&Bytes32::zero()).is_none());
        assert!(!store.has_block(&Bytes32::zero()));
    }

    // -- block_order invariant ---------------------------------------------

    #[test]
    fn block_order_first_seen_invariant_preserved_by_internal_insert() {
        let (state_a, block_a) = anchor_pair(4);
        let anchor_root_a: Bytes32 = block_a.hash_tree_root().into();
        let mut store = Store::from_anchor(state_a, block_a).unwrap();

        // Insert a second distinct (root, block, state) triple.
        let (state_b, block_b) = anchor_pair_at_slot(Slot::new(5), 4);
        let root_b: Bytes32 = block_b.hash_tree_root().into();
        store.insert_block(root_b, block_b.clone(), state_b.clone());
        assert_eq!(store.block_order(), &[anchor_root_a, root_b]);

        // Re-insert the second root with the same payload — must be a no-op.
        store.insert_block(root_b, block_b, state_b);
        assert_eq!(store.block_order(), &[anchor_root_a, root_b]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tick_interval_tests {
    use super::*;
    use proptest::prelude::*;

    use crate::test_fixtures::anchor_pair;

    /// Builds an anchored store and positions the clock at `start` via the
    /// test-only builder.
    fn store_at(start: Time) -> Store {
        let (state, block) = anchor_pair(4);
        Store::from_anchor(state, block)
            .unwrap()
            .with_time_for_test(start)
    }

    // -- Table-driven dispatch -------------------------------------------

    #[test]
    fn tick_advances_one_interval_and_resolves_phase() {
        // (start_time, has_proposal, end_time, end_phase)
        let cases: &[(u64, bool, u64, Phase)] = &[
            // Slot 0 intervals 0..3.
            (0, false, 1, Phase::Idle),
            (1, false, 2, Phase::UpdateSafeTarget),
            (2, false, 3, Phase::AcceptNewVotes),
            // Slot rollover: interval 3 → interval 0 of next slot.
            (3, false, 4, Phase::Proposal),
            (3, true, 4, Phase::Proposal),
            // Further slot rollovers.
            (7, false, 8, Phase::Proposal),
            (7, true, 8, Phase::Proposal),
            (11, true, 12, Phase::Proposal),
            // Within-slot transitions at high time.
            (1_000, false, 1_001, Phase::Idle),
            (1_001, false, 1_002, Phase::UpdateSafeTarget),
            (1_002, false, 1_003, Phase::AcceptNewVotes),
            (1_003, true, 1_004, Phase::Proposal),
        ];
        for &(start, has_proposal, end, phase) in cases {
            let mut store = store_at(Time::new(start));
            store.tick_interval(has_proposal).unwrap_or_else(|e| {
                panic!("tick_interval({has_proposal}) failed at start {start}: {e}")
            });
            assert_eq!(
                store.time(),
                Time::new(end),
                "time after tick from {start} with has_proposal={has_proposal}"
            );
            assert_eq!(store.current_phase(), phase, "phase at {end}");
            assert_eq!(
                store.current_slot(),
                end / INTERVALS_PER_SLOT,
                "slot at {end}"
            );
            assert_eq!(
                store.current_interval(),
                end % INTERVALS_PER_SLOT,
                "interval at {end}"
            );
        }
    }

    // -- Error path ------------------------------------------------------

    #[test]
    fn tick_rejects_time_overflow() {
        let mut store = store_at(Time::new(u64::MAX));
        let err = store.tick_interval(false).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::TimeOverflow {
                time: Time::new(u64::MAX)
            }
        );
    }

    #[test]
    fn overflow_error_path_leaves_state_unchanged() {
        let mut store = store_at(Time::new(u64::MAX));
        let snapshot_time = store.time();
        let _ = store.tick_interval(true).unwrap_err();
        assert_eq!(store.time(), snapshot_time);
    }

    // -- Property tests --------------------------------------------------

    proptest! {
        /// Each `tick_interval` advances `time` by exactly 1 and resolves
        /// `current_phase()` to `Time::new(new_time).phase()`.
        #[test]
        fn tick_state_machine_is_plus_one_with_phase_from_time(
            start in 0_u64..(u64::MAX - 256),
            steps in proptest::collection::vec(any::<bool>(), 1..=64),
        ) {
            let mut store = store_at(Time::new(start));
            for &has_proposal in &steps {
                let before = store.time().get();
                store.tick_interval(has_proposal).unwrap();
                let after = store.time().get();
                prop_assert_eq!(after, before + 1);
                prop_assert_eq!(store.current_phase(), Time::new(after).phase());
            }
        }

        /// Phase classification is periodic with period `INTERVALS_PER_SLOT`.
        #[test]
        fn phase_is_periodic_modulo_intervals_per_slot(
            t in 0_u64..(u64::MAX - INTERVALS_PER_SLOT),
        ) {
            prop_assert_eq!(
                Time::new(t).phase(),
                Time::new(t + INTERVALS_PER_SLOT).phase()
            );
        }
    }
}

// ============================================================================
// process_attestation + validate_attestation
// ============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod attestation_tests {
    use super::*;
    use protocol::{Checkpoint, Slot, ValidatorIndex};
    use ssz::encode;
    use types::Signature;

    use crate::test_fixtures::{pinned_chain, signed_vote, signed_vote_at};

    /// Builds a 4-block linear chain (genesis + 3 children) with 4
    /// validators, pins `latest_justified` at the genesis root, and
    /// positions the clock so `current_vote_slot() == 3` (i.e. votes up to
    /// `Slot::new(4)` are admissible).
    fn store_with_chain_at_slot_3() -> (Store, Vec<Bytes32>) {
        // 3 * INTERVALS_PER_SLOT positions the clock at the start of slot 3.
        pinned_chain(4, 4, Time::new(3 * INTERVALS_PER_SLOT))
    }

    /// Constructs a vote whose source / target / head all point at
    /// `(roots[idx], Slot::new(idx))`, cast in slot `vote_slot`.
    fn self_referential_vote(
        validator: u64,
        roots: &[Bytes32],
        idx: usize,
        vote_slot: u64,
    ) -> SignedAttestation {
        let cp = Checkpoint::new(roots[idx], Slot::new(idx as u64));
        signed_vote(
            ValidatorIndex::new(validator),
            cp,
            cp,
            cp,
            Slot::new(vote_slot),
        )
    }

    // -- process_attestation: from-block branch ----------------------------

    #[test]
    fn from_block_inserts_first_vote_returns_true() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let vote = self_referential_vote(0, &roots, 2, 2);
        assert!(store.process_attestation(vote.clone(), true).unwrap());
        assert_eq!(
            store.latest_known_votes().get(&vote.message.validator_id),
            Some(&vote)
        );
        assert!(store.latest_new_votes().is_empty());
    }

    #[test]
    fn from_block_idempotent_same_slot() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let vote = self_referential_vote(0, &roots, 2, 2);
        assert!(store.process_attestation(vote.clone(), true).unwrap());
        // Second call must observe `>=` and return false without mutation.
        assert!(!store.process_attestation(vote.clone(), true).unwrap());
        assert_eq!(store.latest_known_votes().len(), 1);
    }

    #[test]
    fn from_block_rejects_stale_keeps_existing() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let newer = self_referential_vote(0, &roots, 3, 3);
        let older = self_referential_vote(0, &roots, 2, 2);
        assert!(store.process_attestation(newer.clone(), true).unwrap());
        assert!(!store.process_attestation(older, true).unwrap());
        assert_eq!(
            store.latest_known_votes().get(&newer.message.validator_id),
            Some(&newer)
        );
    }

    #[test]
    fn from_block_accepts_newer_evicts_pending() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let pending = self_referential_vote(0, &roots, 2, 2);
        let on_chain = self_referential_vote(0, &roots, 3, 3);
        assert!(store.process_attestation(pending.clone(), false).unwrap());
        // From-block at strictly newer slot promotes into known AND evicts
        // the stale pending entry (a single `true` return covers both).
        assert!(store.process_attestation(on_chain.clone(), true).unwrap());
        assert_eq!(
            store
                .latest_known_votes()
                .get(&on_chain.message.validator_id),
            Some(&on_chain)
        );
        assert!(store.latest_new_votes().is_empty());
    }

    #[test]
    fn from_block_eviction_compares_attestation_slot_not_target_slot() {
        // PARITY: pending vote has `target.slot = 99`, attestation slot 2.
        // The on-chain vote has attestation slot 3 with `target.slot = 0`.
        // Eviction must compare attestation slot (3 > 2), not target.slot.
        let (mut store, roots) = store_with_chain_at_slot_3();
        let target_late = Checkpoint::new(roots[2], Slot::new(2));
        let source_genesis = Checkpoint::new(roots[0], Slot::ZERO);
        let pending = signed_vote(
            ValidatorIndex::new(0),
            target_late,
            target_late,
            source_genesis,
            Slot::new(2),
        );
        store.insert_new_vote_for_test(pending);

        let target_early = Checkpoint::new(roots[3], Slot::new(3));
        let on_chain = signed_vote(
            ValidatorIndex::new(0),
            target_early,
            target_early,
            source_genesis,
            Slot::new(3),
        );
        store.process_attestation(on_chain, true).unwrap();
        assert!(store.latest_new_votes().is_empty());
    }

    // -- process_attestation: gossip branch --------------------------------

    #[test]
    fn gossip_inserts_first_vote_into_pending() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let vote = self_referential_vote(0, &roots, 2, 2);
        assert!(store.process_attestation(vote.clone(), false).unwrap());
        assert!(store.latest_known_votes().is_empty());
        assert_eq!(
            store.latest_new_votes().get(&vote.message.validator_id),
            Some(&vote)
        );
    }

    #[test]
    fn gossip_idempotent_same_slot() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let vote = self_referential_vote(0, &roots, 2, 2);
        assert!(store.process_attestation(vote.clone(), false).unwrap());
        assert!(!store.process_attestation(vote, false).unwrap());
    }

    #[test]
    fn gossip_no_freshness_gate_vs_current_vote_slot() {
        // current_vote_slot = 3. A vote at slot 4 (= current+1) is admissible
        // — gossip has no extra freshness gate beyond `validate_attestation`.
        let (mut store, roots) = store_with_chain_at_slot_3();
        let vote = self_referential_vote(0, &roots, 3, 4);
        assert!(store.process_attestation(vote, false).unwrap());
    }

    #[test]
    fn resolved_source_root_maps_the_genesis_placeholder_to_the_anchor() {
        let (store, roots) = store_with_chain_at_slot_3();
        let anchor = Checkpoint::new(roots[0], Slot::ZERO);
        let vote = AttestationData {
            slot: Slot::ZERO,
            head: anchor,
            target: anchor,
            source: Checkpoint::default(),
        };

        assert_eq!(store.resolved_source_root(&vote), roots[0]);
    }

    #[test]
    fn resolved_source_root_leaves_a_declared_root_untouched() {
        // The negative case: without it the guard could widen into "always
        // return latest_justified" and every test above would still pass.
        let (store, roots) = store_with_chain_at_slot_3();
        let declared = Checkpoint::new(roots[1], Slot::ONE);
        let vote = AttestationData {
            slot: Slot::ONE,
            head: declared,
            target: declared,
            source: declared,
        };

        assert_eq!(store.resolved_source_root(&vote), roots[1]);
    }

    #[test]
    fn gossip_accepts_genesis_placeholder_source_without_rewriting_it() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let anchor = Checkpoint::new(roots[0], Slot::ZERO);
        let vote = signed_vote(
            ValidatorIndex::new(0),
            anchor,
            anchor,
            Checkpoint::default(),
            Slot::ZERO,
        );

        assert!(store.process_attestation(vote, false).unwrap());
        let stored = store
            .latest_new_votes()
            .get(&ValidatorIndex::new(0))
            .expect("the placeholder-source vote should enter the pending pool");
        // The stored source is the exact INPUT value, not the resolved one — which
        // is what the assertion this replaced could not distinguish, since it
        // compared against the resolved value instead. It covers this ONE field;
        // full-envelope identity is pinned by
        // `signed_attestation_round_trips_byte_identical` below.
        assert_eq!(stored.message.data.source, Checkpoint::default());
    }

    #[test]
    fn placeholder_source_tolerance_does_not_extend_past_slot_zero() {
        // Adversarial-review regression. The tolerance exists for a peer's slot-0
        // bootstrap vote. Before this clause it applied at ANY slot up to the
        // future cap, and because `insert_if_newer` is first-wins at equal slot, a
        // placeholder vote parked at the cap refused every honest vote from that
        // validator at or below it — while also being excluded from produced block
        // bodies, so it moved the head on gossip peers and not on blocks-only ones.
        let (mut store, roots) = store_with_chain_at_slot_3();
        let anchor = Checkpoint::new(roots[0], Slot::ZERO);

        // current_vote_slot() == 3, so slot 4 is the highest admissible vote slot.
        let parked = signed_vote(
            ValidatorIndex::new(0),
            anchor,
            anchor,
            Checkpoint::default(),
            Slot::new(4),
        );
        let err = store
            .process_attestation(parked, false)
            .expect_err("a placeholder source above slot 0 must not resolve");
        assert_eq!(
            err,
            ForkchoiceError::UnknownSourceBlock {
                root: Bytes32::zero()
            },
        );

        // The honest slot-0 shape the tolerance exists for still resolves.
        let bootstrap = signed_vote(
            ValidatorIndex::new(0),
            anchor,
            anchor,
            Checkpoint::default(),
            Slot::ZERO,
        );
        assert!(store.process_attestation(bootstrap, false).unwrap());
    }

    #[test]
    fn signed_attestation_round_trips_byte_identical() {
        // The two pools are written by different statements, so both branches are
        // covered: `true` inserts into `latest_known_votes`, `false` into
        // `latest_new_votes`.
        let cases: [(&str, bool); 2] = [("gossip branch", false), ("on-chain branch", true)];

        for (name, is_from_block) in cases {
            let (mut store, roots) = store_with_chain_at_slot_3();
            let anchor = Checkpoint::new(roots[0], Slot::ZERO);

            // The genesis-placeholder shape: an all-zero source against the anchor.
            // This is the ONLY input the resolution touches, so a vote that did not
            // trigger it would pass before and after the fix.
            let mut before = signed_vote(
                ValidatorIndex::new(0),
                anchor,
                anchor,
                Checkpoint::default(),
                Slot::ZERO,
            );
            // `signed_vote` hardcodes `Signature::zero()`, and an all-zero signature
            // cannot distinguish a preserved one from a dropped one.
            before.signature = Signature::new([0x5a; Signature::LEN]);
            let wire_before = encode(&before);

            assert!(
                store
                    .process_attestation(before.clone(), is_from_block)
                    .expect("the genesis-placeholder source must still resolve"),
                "case {name}: the vote must be accepted, not merely left unmodified",
            );

            let pool = if is_from_block {
                store.latest_known_votes()
            } else {
                store.latest_new_votes()
            };
            let stored = pool.get(&ValidatorIndex::new(0)).unwrap_or_else(|| {
                panic!("case {name}: the accepted vote is missing from the pool")
            });

            assert_eq!(
                encode(stored),
                wire_before,
                "case {name}: the stored envelope must be byte-identical to the one that arrived",
            );
        }
    }

    #[test]
    fn gossip_rejects_non_genesis_zero_source_vote() {
        let (mut store, roots) = store_with_chain_at_slot_3();
        let target = Checkpoint::new(roots[1], Slot::ONE);
        let vote = signed_vote(
            ValidatorIndex::new(0),
            target,
            target,
            Checkpoint::default(),
            Slot::ONE,
        );

        let err = store.process_attestation(vote, false).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::UnknownSourceBlock {
                root: Bytes32::zero()
            }
        );
    }

    // -- lookup_block --------------------------------------------------

    #[test]
    fn lookup_block_returns_tracked_block() {
        let (store, roots) = store_with_chain_at_slot_3();
        let block = store
            .lookup_block(roots[2], |root| ForkchoiceError::UnknownSourceBlock {
                root,
            })
            .unwrap();
        assert_eq!(block.slot, Slot::new(2));
    }

    #[test]
    fn lookup_block_invokes_caller_error_constructor() {
        let (store, _roots) = store_with_chain_at_slot_3();
        let missing = Bytes32::new([0x77; 32]);
        // The closure is what selects the variant; same missing root, two
        // different variants depending on which constructor the caller
        // supplies.
        let err = store
            .lookup_block(missing, |root| ForkchoiceError::UnknownSourceBlock { root })
            .unwrap_err();
        assert_eq!(err, ForkchoiceError::UnknownSourceBlock { root: missing });

        let err = store
            .lookup_block(missing, |root| ForkchoiceError::UnknownTargetBlock { root })
            .unwrap_err();
        assert_eq!(err, ForkchoiceError::UnknownTargetBlock { root: missing });
    }

    // -- validate_attestation: validator bound follows the head registry ---

    #[test]
    fn attestation_bound_follows_head_registry() {
        // The bound is the head post-state's registry length, not a scalar
        // snapshotted at the anchor.
        let (store, roots) = store_with_chain_at_slot_3();
        let target = Checkpoint::new(roots[2], Slot::new(2));
        let source = Checkpoint::new(roots[0], Slot::ZERO);

        let in_range = signed_vote(ValidatorIndex::new(3), target, target, source, Slot::new(2));
        store
            .validate_validator_index(in_range.message.validator_id)
            .unwrap();

        let out_of_range =
            signed_vote(ValidatorIndex::new(4), target, target, source, Slot::new(2));
        let err = store
            .validate_validator_index(out_of_range.message.validator_id)
            .unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::ValidatorIndexOutOfRange {
                validator_id: 4,
                num_validators: 4,
            }
        );
    }

    #[test]
    fn validate_attestation_fails_closed_without_head_state() {
        // A store with no tracked head post-state cannot know the bound. It
        // must REJECT rather than admit the vote — this gate is what stops a
        // peer flooding the pool with arbitrary validator ids.
        let store = Store::default();
        let target = Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::ZERO);
        let sv = signed_vote(ValidatorIndex::new(0), target, target, target, Slot::ZERO);
        let err = store
            .validate_validator_index(sv.message.validator_id)
            .unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::HeadStateNotFound {
                root: Bytes32::zero()
            }
        );
    }

    // -- validate_attestation: rejection paths ----------------------------

    #[test]
    fn validate_unknown_source() {
        let (store, roots) = store_with_chain_at_slot_3();
        let bad_source = Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::ZERO);
        let target = Checkpoint::new(roots[2], Slot::new(2));
        let sv = signed_vote(
            ValidatorIndex::new(0),
            target,
            target,
            bad_source,
            Slot::new(2),
        );
        let err = store.validate_attestation(&sv.message.data).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::UnknownSourceBlock {
                root: bad_source.root
            }
        );
    }

    #[test]
    fn validate_unknown_target() {
        let (store, roots) = store_with_chain_at_slot_3();
        let source = Checkpoint::new(roots[0], Slot::ZERO);
        let bad_target = Checkpoint::new(Bytes32::new([0xbb; 32]), Slot::new(2));
        let sv = signed_vote(
            ValidatorIndex::new(0),
            bad_target,
            bad_target,
            source,
            Slot::new(2),
        );
        let err = store.validate_attestation(&sv.message.data).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::UnknownTargetBlock {
                root: bad_target.root
            }
        );
    }

    #[test]
    fn validate_source_slot_after_target() {
        let (store, roots) = store_with_chain_at_slot_3();
        let source = Checkpoint::new(roots[2], Slot::new(2));
        let target = Checkpoint::new(roots[1], Slot::new(1));
        let sv = signed_vote(ValidatorIndex::new(0), target, target, source, Slot::new(2));
        assert_eq!(
            store.validate_attestation(&sv.message.data).unwrap_err(),
            ForkchoiceError::SourceSlotExceedsTarget
        );
    }

    #[test]
    fn validate_source_checkpoint_slot_mismatch() {
        let (store, roots) = store_with_chain_at_slot_3();
        // Source root resolves to slot 0, but checkpoint claims slot 1.
        let lying_source = Checkpoint::new(roots[0], Slot::new(1));
        let target = Checkpoint::new(roots[2], Slot::new(2));
        let sv = signed_vote(
            ValidatorIndex::new(0),
            target,
            target,
            lying_source,
            Slot::new(2),
        );
        assert_eq!(
            store.validate_attestation(&sv.message.data).unwrap_err(),
            ForkchoiceError::SourceCheckpointSlotMismatch
        );
    }

    #[test]
    fn validate_target_checkpoint_slot_mismatch() {
        let (store, roots) = store_with_chain_at_slot_3();
        let source = Checkpoint::new(roots[0], Slot::ZERO);
        // Target root resolves to slot 2, but checkpoint claims slot 3.
        let lying_target = Checkpoint::new(roots[2], Slot::new(3));
        let sv = signed_vote(
            ValidatorIndex::new(0),
            lying_target,
            lying_target,
            source,
            Slot::new(3),
        );
        assert_eq!(
            store.validate_attestation(&sv.message.data).unwrap_err(),
            ForkchoiceError::TargetCheckpointSlotMismatch
        );
    }

    #[test]
    fn validate_rejects_attestation_beyond_plus_one() {
        // current_vote_slot = 3; limit = 4. Vote at slot 5 must be rejected.
        let (store, roots) = store_with_chain_at_slot_3();
        let sv = signed_vote_at(
            ValidatorIndex::new(0),
            roots[3],
            Slot::new(3),
            Slot::new(5),
            Checkpoint::new(roots[0], Slot::ZERO),
        );
        assert_eq!(
            store.validate_attestation(&sv.message.data).unwrap_err(),
            ForkchoiceError::AttestationTooFarInFuture {
                vote_slot: Slot::new(5),
                limit: Slot::new(4),
            }
        );
    }

    // NOTE: `ForkchoiceError::AttestationFutureLimitOverflow` is unreachable
    // through normal clock advancement — `current_vote_slot = time /
    // INTERVALS_PER_SLOT` is bounded by `u64::MAX / 4`, so `Slot::advance`
    // always succeeds. The variant is retained as defense-in-depth for
    // future `Slot` constructors that bypass the clock.

    // -- validate_attestation: the full predicate set ----------------------

    /// One case in [`validate_attestation_predicate_matrix`]. Named fields
    /// rather than a six-wide tuple: a positional row of three `Checkpoint`s
    /// reads as `(b2, b1, anchor)` at the call site, where nothing distinguishes
    /// head from target from source.
    struct PredicateCase {
        name: &'static str,
        head: Checkpoint,
        target: Checkpoint,
        source: Checkpoint,
        vote_slot: Slot,
        /// `None` asserts acceptance; `Some(err)` asserts the exact variant.
        want: Option<ForkchoiceError>,
    }

    /// Accept/reject coverage for the nine predicates leanSpec asserts in
    /// `forkchoice/store.py:299-:325 @ 0c9528ac`.
    ///
    /// ONE accept row covers eight of the nine by construction — a vote that
    /// satisfies everything is simultaneously the accept side of P1-P4 and
    /// P6-P9, and nine near-identical accept rows would be duplication rather
    /// than coverage. P5 earns a second accept row because its boundary
    /// (`head.slot == target.slot`, the equality edge of `>=`) is a distinct
    /// input from the strict-inequality baseline.
    ///
    /// Every reject row isolates its own predicate: a vote that violates two
    /// proves only that the earlier group fires.
    /// The eleven rows, built from a chain fixture's roots. Split out so the
    /// assertion loop stays readable — and so the table can be read as data.
    // `too_many_lines` targets branching complexity; this function has none — it
    // is one array literal of eleven data rows, already split out of the test so
    // the assertion loop stays short. Compressing it further would mean either
    // dropping the per-row names that make a failure readable or splitting the
    // spec's four predicate groups across two builders, both of which cost more
    // than the lint buys here.
    #[allow(clippy::too_many_lines)]
    fn predicate_cases(roots: &[Bytes32]) -> [PredicateCase; 11] {
        let anchor = Checkpoint::new(roots[0], Slot::ZERO);
        let b1 = Checkpoint::new(roots[1], Slot::ONE);
        let b2 = Checkpoint::new(roots[2], Slot::new(2));
        let untracked = Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::ZERO);
        // A second untracked checkpoint, declared at a slot that SATISFIES P5
        // against the target. Reusing the slot-0 one as a head would trip P5 as
        // well as P3, and the row would then prove only that availability is
        // checked before topology — the group order it is supposed to be
        // independent of.
        let untracked_head = Checkpoint::new(Bytes32::new([0xab; 32]), Slot::new(2));

        // Expected variants, hoisted so each row below reads as one line of data.
        let no_source = ForkchoiceError::UnknownSourceBlock {
            root: untracked.root,
        };
        let no_target = ForkchoiceError::UnknownTargetBlock {
            root: untracked.root,
        };
        let no_head = ForkchoiceError::UnknownAttestationHeadBlock {
            root: untracked_head.root,
        };
        let head_below = ForkchoiceError::HeadSlotBelowTarget {
            head_slot: Slot::ONE,
            target_slot: Slot::new(2),
        };
        let too_far = ForkchoiceError::AttestationTooFarInFuture {
            vote_slot: Slot::new(5),
            limit: Slot::new(4),
        };

        // The clock sits at slot 3, so slot 4 is the highest admissible vote slot.
        let vote_slot = Slot::new(2);
        let case = |name, head, target, source, want| PredicateCase {
            name,
            head,
            target,
            source,
            vote_slot,
            want,
        };

        [
            case("accept: all nine satisfied", b2, b1, anchor, None),
            case(
                "P1 reject: source untracked",
                b2,
                b1,
                untracked,
                Some(no_source),
            ),
            case(
                "P2 reject: target untracked",
                b2,
                untracked,
                anchor,
                Some(no_target),
            ),
            case(
                "P3 reject: head untracked",
                untracked_head,
                b1,
                anchor,
                Some(no_head),
            ),
            case(
                "P4 reject: source slot exceeds target",
                b2,
                anchor,
                b1,
                Some(ForkchoiceError::SourceSlotExceedsTarget),
            ),
            case(
                "P5 reject: head older than target",
                b1,
                b2,
                anchor,
                Some(head_below),
            ),
            case(
                "P5 accept: head at the target's own slot",
                b1,
                b1,
                anchor,
                None,
            ),
            case(
                "P6 reject: source checkpoint slot mismatch",
                b2,
                b1,
                Checkpoint::new(roots[0], Slot::ONE),
                Some(ForkchoiceError::SourceCheckpointSlotMismatch),
            ),
            case(
                "P7 reject: target checkpoint slot mismatch",
                b2,
                Checkpoint::new(roots[1], Slot::new(2)),
                anchor,
                Some(ForkchoiceError::TargetCheckpointSlotMismatch),
            ),
            case(
                "P8 reject: head checkpoint slot mismatch",
                Checkpoint::new(roots[2], Slot::ONE),
                b1,
                anchor,
                Some(ForkchoiceError::HeadCheckpointSlotMismatch),
            ),
            PredicateCase {
                name: "P9 reject: vote slot beyond current + 1",
                head: b2,
                target: b1,
                source: anchor,
                vote_slot: Slot::new(5),
                want: Some(too_far),
            },
        ]
    }

    #[test]
    fn validate_attestation_predicate_matrix() {
        let (store, roots) = store_with_chain_at_slot_3();

        for PredicateCase {
            name,
            head,
            target,
            source,
            vote_slot,
            want,
        } in predicate_cases(&roots)
        {
            let sv = signed_vote(ValidatorIndex::new(0), head, target, source, vote_slot);
            let got = store.validate_attestation(&sv.message.data);
            match (want, got) {
                (None, Ok(())) => {}
                (None, Err(err)) => panic!("case {name}: expected accept, got {err:?}"),
                (Some(want), Err(err)) => {
                    assert_eq!(err, want, "case {name}: wrong rejection variant");
                }
                (Some(want), Ok(())) => {
                    panic!("case {name}: expected rejection {want:?}, got accept")
                }
            }
        }
    }

    #[test]
    fn process_attestation_bounds_the_validator_before_touching_blocks() {
        // The validator-index guard must run AHEAD of the block lookups. This
        // store has neither a head post-state nor any tracked block, so the two
        // orders report different errors: guard-first fails closed with
        // HeadStateNotFound, lookup-first would report UnknownSourceBlock. That
        // difference is the whole assertion — it is the only test that fails if
        // the two calls in `process_attestation` are swapped.
        let mut store = Store::default();
        let cp = Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::ZERO);
        let sv = signed_vote(ValidatorIndex::new(0), cp, cp, cp, Slot::ZERO);

        assert_eq!(
            store.process_attestation(sv, false).unwrap_err(),
            ForkchoiceError::HeadStateNotFound {
                root: Bytes32::zero()
            },
        );
    }

    #[test]
    fn an_accepted_vote_with_a_distinct_head_enters_the_pending_pool() {
        // The matrix asserts `validate_attestation` returns Ok; this asserts the
        // accepted vote actually lands where `process_attestation` says it does.
        // Every other pool test uses `self_referential_vote`, which collapses
        // head == target == source and so satisfies P3, P5 and P8 by
        // construction — none of them shows that a vote with a DISTINCT,
        // correctly-slotted head still enters the pool.
        let (mut store, roots) = store_with_chain_at_slot_3();
        let anchor = Checkpoint::new(roots[0], Slot::ZERO);
        let b1 = Checkpoint::new(roots[1], Slot::ONE);
        let b2 = Checkpoint::new(roots[2], Slot::new(2));
        let sv = signed_vote(ValidatorIndex::new(0), b2, b1, anchor, Slot::new(2));

        assert!(store.process_attestation(sv.clone(), false).unwrap());
        assert_eq!(
            store.latest_new_votes().get(&ValidatorIndex::new(0)),
            Some(&sv),
            "an accepted vote must reach the pending pool unchanged",
        );
    }

    /// Fork choice weighs participation from plain `Attestation` data only —
    /// never from `signature` bytes. Two independent stores over the identical
    /// chain are fed the identical `Attestation` payloads but differing
    /// signatures; every vote must be admitted and retained in both, and both
    /// must resolve the same head. This is the crypto-free guarantee (no
    /// signature verify on any fork-choice path).
    #[test]
    fn participation_ignores_signature_bytes() {
        // Runs the same 4-voter chain under one fixed signature value; returns
        // the resolved head and the retained-vote count.
        let run = |sig: Signature| -> (Bytes32, usize) {
            let (mut store, roots) = store_with_chain_at_slot_3();
            for v in 0..4 {
                let mut vote = self_referential_vote(v, &roots, 2, 2);
                vote.signature = sig.clone();
                // Gossip branch → latest_new_votes. The `true` return proves
                // the vote was admitted, NOT rejected on signature content.
                assert!(store.process_attestation(vote, false).unwrap());
            }
            store.accept_new_votes().unwrap();
            (store.head(), store.latest_known_votes().len())
        };

        let (head_zero, kept_zero) = run(Signature::zero());
        let (head_filled, kept_filled) = run(Signature::new([0x7u8; Signature::LEN]));

        // Non-vacuous: every vote admitted and retained regardless of signature.
        assert_eq!(kept_zero, 4);
        assert_eq!(kept_filled, 4);
        // Identical plain data → identical head, independent of signature bytes.
        assert_eq!(head_zero, head_filled);
    }
}

// ============================================================================
// update_safe_target + accept_new_votes (update_head)
// ============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod update_safe_target_tests {
    use super::*;
    use protocol::{Checkpoint, Slot, ValidatorIndex};

    use crate::test_fixtures::{pinned_chain, signed_vote, signed_vote_at};

    /// Builds a linear chain pinned to genesis-justified at the genesis
    /// clock value.
    fn chain_pinned_to_genesis(n_blocks: u64, num_validators: u64) -> (Store, Vec<Bytes32>) {
        pinned_chain(n_blocks, num_validators, Time::ZERO)
    }

    fn vote_for(validator: u64, roots: &[Bytes32], head_idx: usize) -> SignedAttestation {
        signed_vote_at(
            ValidatorIndex::new(validator),
            roots[head_idx],
            Slot::new(head_idx as u64),
            Slot::new(head_idx as u64),
            Checkpoint::new(roots[0], Slot::ZERO),
        )
    }

    #[test]
    fn supermajority_advances_to_voted_head() {
        // 4 validators → ceil(2*4/3) = 3. Three voters at b2 → safe_target = b2.
        let (mut store, roots) = chain_pinned_to_genesis(3, 4);
        for v in 0..3 {
            store.insert_new_vote_for_test(vote_for(v, &roots, 2));
        }
        store.update_safe_target().unwrap();
        assert_eq!(store.safe_target(), roots[2]);
    }

    #[test]
    fn below_supermajority_keeps_safe_target_unchanged() {
        // 4 validators → threshold 3. Only 2 voters → safe_target falls back
        // to the descent origin (latest_justified.root == genesis).
        let (mut store, roots) = chain_pinned_to_genesis(3, 4);
        for v in 0..2 {
            store.insert_new_vote_for_test(vote_for(v, &roots, 2));
        }
        store.update_safe_target().unwrap();
        assert_eq!(store.safe_target(), roots[0]);
    }

    #[test]
    fn exactly_two_thirds_advances() {
        // 6 validators → ceil(12/3) = 4. Exactly 4 voters → advances.
        let (mut store, roots) = chain_pinned_to_genesis(3, 6);
        for v in 0..4 {
            store.insert_new_vote_for_test(vote_for(v, &roots, 2));
        }
        store.update_safe_target().unwrap();
        assert_eq!(store.safe_target(), roots[2]);
    }

    #[test]
    fn just_below_two_thirds_unchanged() {
        // 6 validators → threshold 4. Three voters → unchanged.
        let (mut store, roots) = chain_pinned_to_genesis(3, 6);
        for v in 0..3 {
            store.insert_new_vote_for_test(vote_for(v, &roots, 2));
        }
        store.update_safe_target().unwrap();
        assert_eq!(store.safe_target(), roots[0]);
    }

    #[test]
    fn no_votes_safe_target_is_latest_justified_root() {
        let (mut store, roots) = chain_pinned_to_genesis(3, 4);
        store.update_safe_target().unwrap();
        assert_eq!(store.safe_target(), roots[0]);
    }

    #[test]
    fn scoring_uses_head_not_target_checkpoint() {
        // PARITY: voters declare `head = roots[3]` but `target = roots[1]`.
        // Weight must route to roots[3]'s subtree, not roots[1]'s.
        let (mut store, roots) = chain_pinned_to_genesis(4, 4);
        let source = Checkpoint::new(roots[0], Slot::ZERO);
        let head = Checkpoint::new(roots[3], Slot::new(3));
        let target = Checkpoint::new(roots[1], Slot::new(1));
        for v in 0..3 {
            store.insert_new_vote_for_test(signed_vote(
                ValidatorIndex::new(v),
                head,
                target,
                source,
                Slot::new(3),
            ));
        }
        store.update_safe_target().unwrap();
        assert_eq!(store.safe_target(), roots[3]);
    }

    // -- accept_new_votes drives update_head -------------------------------

    #[test]
    fn accept_new_votes_promotes_pending_and_refreshes_head() {
        let (mut store, roots) = chain_pinned_to_genesis(3, 4);
        // Seed all 4 voters in latest_new_votes pointing at b2.
        for v in 0..4 {
            store.insert_new_vote_for_test(vote_for(v, &roots, 2));
        }
        // Before promotion, head is still genesis (set by from_anchor).
        assert_eq!(store.head(), roots[0]);
        store.accept_new_votes().unwrap();
        assert!(store.latest_new_votes().is_empty());
        assert_eq!(store.latest_known_votes().len(), 4);
        // After promotion + update_head, head switches to b2.
        assert_eq!(store.head(), roots[2]);
    }

    #[test]
    fn accept_new_votes_on_empty_pending_still_refreshes_head() {
        // No pending votes, no known votes: min-score-zero head selection
        // still walks the block tree and applies the canonical
        // zero-weight tie-break.
        let (mut store, roots) = chain_pinned_to_genesis(3, 4);
        store.accept_new_votes().unwrap();
        assert_eq!(store.head(), roots[2]);
    }
}

// ============================================================================
// track_block + get_proposal_head + get_vote_target
// ============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod store_extensions_tests {
    use super::*;
    use protocol::{BlockBody, Slot, ValidatorIndex};

    use crate::test_fixtures::{genesis_store, linear_chain, pinned_chain, signed_vote_at};

    // -- track_block ----------------------------------------------------

    /// Builds a synthetic post-state by cloning the genesis state — fine
    /// for tracking-invariant tests, which never run a state transition
    /// against this state.
    fn fresh_block_and_state(parent_root: Bytes32, slot: u64) -> (Block, State) {
        let (_, anchor_block) = crate::test_fixtures::genesis_anchor(4);
        let state = crate::test_fixtures::genesis_anchor(4).0;
        let state_root: Bytes32 = state.hash_tree_root().into();
        let block = Block {
            slot: Slot::new(slot),
            proposer_index: ValidatorIndex::new(slot % 4),
            parent_root,
            state_root,
            body: BlockBody::default(),
        };
        let _ = anchor_block; // suppress unused warning
        (block, state)
    }

    #[test]
    fn track_block_rejects_state_root_mismatch() {
        let (mut store, anchor_root) = genesis_store(4);
        let (mut block, state) = fresh_block_and_state(anchor_root, 1);
        block.state_root = Bytes32::new([0xff; 32]);
        let err = store.track_block(block, state).unwrap_err();
        assert!(matches!(
            err,
            ForkchoiceError::BlockStateRootMismatch { .. }
        ));
    }

    #[test]
    fn track_block_dedupes_known_root() {
        let (mut store, anchor_root) = genesis_store(4);
        let (block, state) = fresh_block_and_state(anchor_root, 1);
        assert!(store.track_block(block.clone(), state.clone()).unwrap());
        // Second call with same block → Ok(false), no mutation.
        assert!(!store.track_block(block, state).unwrap());
    }

    #[test]
    fn track_block_rejects_missing_parent() {
        let (mut store, _) = genesis_store(4);
        let bogus_parent = Bytes32::new([0xaa; 32]);
        let (block, state) = fresh_block_and_state(bogus_parent, 1);
        let err = store.track_block(block, state).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::ParentBlockNotFound { root: bogus_parent }
        );
    }

    #[test]
    fn track_block_inserts_and_extends_block_order() {
        let (mut store, anchor_root) = genesis_store(4);
        let (block, state) = fresh_block_and_state(anchor_root, 1);
        let new_root: Bytes32 = block.hash_tree_root().into();
        assert!(store.track_block(block, state).unwrap());
        assert!(store.has_block(&new_root));
        assert_eq!(store.block_order().last(), Some(&new_root));
    }

    #[test]
    fn track_block_adopts_newer_known_post_state_checkpoints() {
        let (mut store, roots, states) = linear_chain(2, 2);
        let justified = Checkpoint::new(roots[1], Slot::ONE);
        let finalized = Checkpoint::new(roots[1], Slot::ONE);
        let mut post_state = states[0].clone();
        post_state.latest_justified = justified;
        post_state.latest_finalized = finalized;

        let block = Block {
            slot: Slot::new(2),
            proposer_index: ValidatorIndex::new(0),
            parent_root: roots[1],
            state_root: post_state.hash_tree_root().into(),
            body: BlockBody::default(),
        };

        assert!(store.track_block(block, post_state).unwrap());
        assert_eq!(store.latest_justified(), justified);
        assert_eq!(store.latest_finalized(), finalized);

        let produced = store.produce_attestation_vote(Slot::new(2)).unwrap();
        assert_eq!(produced.source, justified);
    }

    #[test]
    fn track_block_ignores_newer_unknown_post_state_checkpoint() {
        let (mut store, anchor_root) = genesis_store(2);
        let original_justified = store.latest_justified();
        let mut post_state = crate::test_fixtures::genesis_anchor(2).0;
        post_state.latest_justified = Checkpoint::new(Bytes32::new([0x99; 32]), Slot::new(1));

        let block = Block {
            slot: Slot::new(1),
            proposer_index: ValidatorIndex::new(1),
            parent_root: anchor_root,
            state_root: post_state.hash_tree_root().into(),
            body: BlockBody::default(),
        };

        assert!(store.track_block(block, post_state).unwrap());
        assert_eq!(store.latest_justified(), original_justified);
    }

    // -- get_proposal_head ----------------------------------------------

    #[test]
    fn get_proposal_head_returns_current_head_and_promotes_pending() {
        let (mut store, roots) = pinned_chain(3, 4, Time::ZERO);
        // Seed pending votes that would shift head to roots[2] once promoted.
        let source = Checkpoint::new(roots[0], Slot::ZERO);
        for v in 0..4 {
            store.insert_new_vote_for_test(signed_vote_at(
                ValidatorIndex::new(v),
                roots[2],
                Slot::new(2),
                Slot::new(2),
                source,
            ));
        }
        let head = store.get_proposal_head().unwrap();
        assert_eq!(head, roots[2]);
        // accept_new_votes promoted the pending set.
        assert!(store.latest_new_votes().is_empty());
    }

    // -- get_vote_target ------------------------------------------------

    #[test]
    fn get_vote_target_returns_head_when_aligned_with_safe_target() {
        let (store, anchor_root) = genesis_store(4);
        // Initial state: head == safe_target == anchor; walk is a no-op.
        let target = store.get_vote_target().unwrap();
        assert_eq!(target.root, anchor_root);
        assert_eq!(target.slot, Slot::ZERO);
    }

    #[test]
    fn get_vote_target_walks_three_hops_when_head_far_above_safe_target() {
        // Build a 5-block chain. Move head to roots[4] (slot 4) while
        // safe_target stays at roots[0] (slot 0). Target must walk 3 hops
        // → settle at roots[1] (slot 1).
        let (mut store, roots) = pinned_chain(5, 4, Time::ZERO);
        // Seed votes so update_head moves the head to roots[4].
        let source = Checkpoint::new(roots[0], Slot::ZERO);
        for v in 0..4 {
            store.insert_new_vote_for_test(signed_vote_at(
                ValidatorIndex::new(v),
                roots[4],
                Slot::new(4),
                Slot::new(4),
                source,
            ));
        }
        store.accept_new_votes().unwrap();
        assert_eq!(store.head(), roots[4]);

        let target = store.get_vote_target().unwrap();
        assert_eq!(target.root, roots[1]);
        assert_eq!(target.slot, Slot::new(1));
    }

    #[test]
    fn get_vote_target_caps_at_safe_target_when_within_three_hops() {
        // 3-block chain. Head moves to roots[2] (slot 2); safe_target stays
        // at roots[0] (slot 0). After one hop the cursor is at roots[1]
        // (slot 1); after two hops at roots[0] (slot 0) == safe_target.
        // The walk stops there (loop breaks on `<=`), not at hop count 3.
        let (mut store, roots) = pinned_chain(3, 4, Time::ZERO);
        let source = Checkpoint::new(roots[0], Slot::ZERO);
        for v in 0..4 {
            store.insert_new_vote_for_test(signed_vote_at(
                ValidatorIndex::new(v),
                roots[2],
                Slot::new(2),
                Slot::new(2),
                source,
            ));
        }
        store.accept_new_votes().unwrap();
        let target = store.get_vote_target().unwrap();
        assert_eq!(target.root, roots[0]);
        assert_eq!(target.slot, Slot::ZERO);
    }

    #[test]
    fn get_vote_target_unknown_head_errors() {
        let (mut store, _) = genesis_store(4);
        store.head = Bytes32::new([0xff; 32]);
        let err = store.get_vote_target().unwrap_err();
        assert!(matches!(err, ForkchoiceError::UnknownHeadBlock { .. }));
    }

    // -- get_vote_target: the justifiability walk --------------------------

    #[test]
    fn target_selection_matches_spec() {
        // The fixture is chosen so the two walks DISAGREE, which is the only
        // shape that can tell walk 2 apart from no walk 2 at all.
        //
        // Chain of 11 blocks at slots 0..=10, finalization and the safe target
        // both at genesis, head at slot 10.
        //   walk 1: three hops (JUSTIFICATION_LOOKBACK_SLOTS) -> slot 7
        //   slot 7 is NOT justifiable after slot 0: delta 7 is not <= 5, not a
        //   perfect square, not pronic (crates/protocol/src/slot.rs)
        //   walk 2: one hop -> slot 6, delta 6 = 2*3, pronic -> justifiable
        let (mut store, roots) = pinned_chain(11, 4, Time::new(10 * INTERVALS_PER_SLOT));
        // No test-only setters: `accept_new_votes` refreshes the head through
        // the normal LMD-GHOST walk, and `from_anchor` already seeded the safe
        // target and the finalized checkpoint at genesis.
        store.accept_new_votes().expect("head refresh");
        assert_eq!(
            store.head(),
            roots[10],
            "fixture precondition: head at slot 10"
        );
        assert_eq!(
            store.safe_target(),
            roots[0],
            "fixture precondition: safe target at genesis"
        );
        assert_eq!(store.latest_finalized().slot, Slot::ZERO);

        let target = store.get_vote_target().expect("target selection");

        assert_eq!(
            target,
            Checkpoint::new(roots[6], Slot::new(6)),
            "walk 2 must step back from the walk-1 landing at slot 7 to slot 6",
        );
        // The two properties the assertion above depends on, pinned separately so
        // a failure says WHICH one broke.
        assert!(
            !Slot::new(7).is_justifiable_after(Slot::ZERO),
            "fixture is vacuous unless the walk-1 landing is non-justifiable",
        );
        assert!(
            target.slot.is_justifiable_after(Slot::ZERO),
            "the selected slot must be justifiable after the finalized slot",
        );
    }

    #[test]
    fn target_selection_stops_at_the_finalized_checkpoint() {
        // Regression for the store shape that made a literal port of the spec
        // walk step off the bottom of the chain: `adopt_post_state_checkpoints`
        // advances `latest_finalized` without refreshing `safe_target`, so walk 1
        // can descend BELOW the finalized slot, where `is_justifiable_after` is
        // false for every ancestor and the walk runs out of chain.
        let (mut store, roots, states) = linear_chain(2, 2);
        let checkpoint = Checkpoint::new(roots[1], Slot::ONE);
        let mut post_state = states[0].clone();
        post_state.latest_justified = checkpoint;
        post_state.latest_finalized = checkpoint;

        let block = Block {
            slot: Slot::new(2),
            proposer_index: ValidatorIndex::new(0),
            parent_root: roots[1],
            state_root: post_state.hash_tree_root().into(),
            body: BlockBody::default(),
        };
        store.track_block(block, post_state).expect("track");
        store.accept_new_votes().expect("head refresh");

        // safe_target is still genesis (slot 0) while finalization is at slot 1.
        assert_eq!(store.safe_target(), roots[0]);
        let target = store
            .get_vote_target()
            .expect("must not walk past finalized");
        assert_eq!(
            target, checkpoint,
            "the walk must stop at the finalized checkpoint, not step past genesis",
        );
    }

    #[test]
    fn target_selection_undershoots_when_finalized_is_off_ancestry() {
        // The floor bounds TERMINATION, not the post-condition. When the
        // finalized checkpoint sits on a branch the head does not descend from,
        // one parent step can skip from above the finalized slot to below it and
        // both loops exit on the floor with a NON-justifiable target. This test
        // pins that as deliberate documented behaviour rather than an accident;
        // the divergence that produces it is tracked as its own issue.
        let (mut store, anchor) = genesis_store(4);

        // Two children of genesis at different slots, so whichever is NOT the
        // head sits at a slot the head's ancestry does not contain.
        let (block_a, state_a) = fresh_block_and_state(anchor, 4);
        let (block_b, state_b) = fresh_block_and_state(anchor, 2);
        let root_a: Bytes32 = block_a.hash_tree_root().into();
        let root_b: Bytes32 = block_b.hash_tree_root().into();
        store.track_block(block_a, state_a).expect("track a");
        store.track_block(block_b, state_b).expect("track b");
        store.accept_new_votes().expect("head refresh");

        // The head is decided by the (weight, root) tie-break, so the test READS
        // it rather than assuming it, and finalizes the other branch. Both
        // orientations undershoot: a head above the finalized slot walks past it,
        // and a head below it is already under the floor.
        let head = store.head();
        let (off_root, off_slot) = if head == root_a {
            (root_b, Slot::new(2))
        } else {
            assert_eq!(head, root_b, "head must be one of the two children");
            (root_a, Slot::new(4))
        };

        // Adopt the off-ancestry checkpoint the way ordinary operation does: a
        // block on that branch whose post-state names it finalized. Built by hand
        // because `state_root` must commit to the MODIFIED state — the same
        // reason `track_block_adopts_newer_known_post_state_checkpoints` does.
        let mut post_state = crate::test_fixtures::genesis_anchor(4).0;
        post_state.latest_finalized = Checkpoint::new(off_root, off_slot);
        let follower = Block {
            slot: Slot::new(5),
            proposer_index: ValidatorIndex::new(1),
            parent_root: off_root,
            state_root: post_state.hash_tree_root().into(),
            body: BlockBody::default(),
        };
        store
            .track_block(follower, post_state)
            .expect("track the follower that carries the finalized checkpoint");
        assert_eq!(store.latest_finalized().root, off_root);
        assert_eq!(store.head(), head, "tracking must not move the head");

        let target = store.get_vote_target().expect("the walk must still return");
        assert!(
            target.slot < store.latest_finalized().slot,
            "documented undershoot: the target sits below the finalized slot",
        );
        assert!(
            !target
                .slot
                .is_justifiable_after(store.latest_finalized().slot),
            "and it is NOT justifiable — the residual this test exists to make visible",
        );
    }
}
