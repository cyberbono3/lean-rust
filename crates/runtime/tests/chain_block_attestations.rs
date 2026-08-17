//! Block-carried attestations must reach the fork-choice store.
//!
//! Both tests here share one fork: two sibling blocks on the genesis anchor,
//! and a set of attestations backing the sibling the equal-weight tie-break
//! does NOT prefer. That construction is what makes the assertions
//! discriminating — a fixture backing the sibling that already wins would pass
//! before the fold existed.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use protocol::{
    Attestation, AttestationData, Checkpoint, SignedAttestation, SignedBlockWithAttestation, Slot,
    ValidatorIndex,
};
use runtime::chain::engine::test_fixtures::{
    engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS,
};
use runtime::chain::engine::{AttestationImportResult, BlockImportResult, Engine};
use ssz::HashTreeRoot;
use types::{Bytes32, Signature};

/// The fork every test here runs on.
struct Fork {
    /// Both siblings, ready to import into any engine.
    siblings: [SignedBlockWithAttestation; 2],
    /// The sibling the equal-weight tie-break does NOT prefer, and therefore
    /// the one the carried votes must be able to promote.
    backed: Bytes32,
    /// The sibling the tie-break prefers with no votes present.
    unbacked: Bytes32,
    /// Attestations backing `backed`, one per voting validator.
    votes: Vec<SignedAttestation>,
}

/// Builds two sibling blocks on the genesis anchor and the attestations that
/// back the lexicographically smaller of the two roots.
///
/// Two engines are used only as block factories: each produces one child of the
/// anchor without ever seeing the other's block, which is the only way to get
/// two blocks that share a parent.
fn fork() -> Fork {
    let factory_a = engine_at_genesis(ENGINE_VALIDATORS);
    let factory_b = engine_at_genesis(ENGINE_VALIDATORS);
    let anchor = factory_a.head();

    let block_a = produce_signed_block(&factory_a, Slot::new(1), ValidatorIndex::new(1));
    let block_b = produce_signed_block(&factory_b, Slot::new(2), ValidatorIndex::new(2));

    assert_eq!(
        block_a.message.block.parent_root,
        anchor,
        "fixture precondition: block_a must be a child of the anchor",
    );
    assert_eq!(
        block_b.message.block.parent_root,
        anchor,
        "fixture precondition: block_b must be a child of the anchor",
    );

    let root_a: Bytes32 = block_a.message.block.hash_tree_root().into();
    let root_b: Bytes32 = block_b.message.block.hash_tree_root().into();
    assert_ne!(root_a, root_b, "the two siblings must be distinct blocks");

    // Equal weight resolves on the larger root (the fork-choice descent keys on
    // `(weight, root)`), so back the smaller one: the votes then have to do real
    // work. The slot rides along with the root — `backed` may be either sibling,
    // and they sit at different slots, so a hardcoded value would disagree with
    // the block it names.
    let (backed, backed_slot, unbacked) = if root_a < root_b {
        (root_a, Slot::new(1), root_b)
    } else {
        (root_b, Slot::new(2), root_a)
    };

    // The anchor is both source and target; only `head` distinguishes the vote.
    // `validate_attestation` checks source and target, never head.
    let anchor_cp = Checkpoint::new(anchor, Slot::ZERO);
    let backed_cp = Checkpoint::new(backed, backed_slot);
    let votes = (0..ENGINE_VALIDATORS - 1)
        .map(|v| SignedAttestation {
            message: Attestation::new(
                ValidatorIndex::new(v),
                AttestationData {
                    slot: Slot::new(1),
                    head: backed_cp,
                    target: anchor_cp,
                    source: anchor_cp,
                },
            ),
            signature: Signature::default(),
        })
        .collect();

    Fork {
        siblings: [block_a, block_b],
        backed,
        unbacked,
        votes,
    }
}

/// Imports both siblings into a fresh engine and returns it. The resulting head
/// is the tie-break winner, because no votes have been seen.
fn engine_with_fork(fork: &Fork) -> Engine {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    for block in &fork.siblings {
        assert!(matches!(
            engine.import_block(block.clone()),
            BlockImportResult::Accepted { .. }
        ));
    }
    assert_eq!(
        engine.head(),
        fork.unbacked,
        "precondition: with no votes the tie-break prefers the unbacked sibling",
    );
    engine
}

/// Builds the block that carries the fork's votes: an engine that has seen both
/// siblings AND the gossip votes produces a child of the backed sibling, so the
/// production path folds those votes into its body.
fn carrier_block(fork: &Fork) -> SignedBlockWithAttestation {
    let producer = engine_with_fork(fork);
    for vote in &fork.votes {
        // `AttestationImportResult` is `#[must_use]`, so the outcome cannot be
        // discarded — and asserting it is stronger anyway: a vote silently
        // `Ignored` here would leave the carrier body short and every assertion
        // downstream vacuous.
        assert!(
            matches!(
                producer.import_attestation(vote.clone()),
                AttestationImportResult::Accepted { .. }
            ),
            "gossip vote must be admitted to the pending pool",
        );
    }
    // NOT `producer.head()` here. The gossip branch only inserts into the
    // pending pool and never refreshes the head, so at this point the head is
    // still `fork.unbacked`. What IS true is that every vote landed in the
    // pending pool.
    assert_eq!(
        producer.with_store(|s| s.latest_new_votes().len()),
        fork.votes.len(),
        "every gossip vote must have been admitted to the pending pool",
    );

    let block = produce_signed_block(&producer, Slot::new(3), ValidatorIndex::new(3));

    // Assert the PARENT, not the head. Production promotes — `produce_block`
    // resolves its parent through the proposal-head path, which calls
    // `accept_new_votes` — so a carrier parented on the backed sibling is proof
    // the pending votes were counted. `producer.head()` is NOT that proof:
    // finalising the produced block tracks it and refreshes the head again, so
    // by the time this returns the head has descended PAST `fork.backed` onto
    // the carrier itself.
    assert_eq!(
        block.message.block.parent_root, fork.backed,
        "producing must have promoted the pending votes, so the carrier is a \
         child of the backed sibling",
    );
    assert!(
        !block.message.block.body.attestations.is_empty(),
        "the carrier block must actually carry the votes; an empty body would \
         make every assertion downstream vacuous",
    );
    block
}

#[test]
fn block_carried_attestations_affect_head_weight() {
    let fork = fork();
    let carrier = carrier_block(&fork);

    // A node that has seen both siblings and NO attestations.
    let node = engine_with_fork(&fork);
    let before = node.with_store(|s| s.latest_known_votes().len());
    assert_eq!(before, 0, "precondition: the node has seen no votes");

    assert!(matches!(
        node.import_block(carrier.clone()),
        BlockImportResult::Accepted { .. }
    ));

    // The votes reached the KNOWN pool — not nowhere.
    let known = node.with_store(|s| s.latest_known_votes().len());
    assert_eq!(
        known,
        fork.votes.len(),
        "every vote the block carried must be in the known pool, and the head \
         assertion below is vacuous without it. A short count has two causes: \
         check carrier.message.block.body.attestations.len() first, because a \
         body that carried fewer votes than the fixture built looks identical \
         here to validate_attestation rejecting them",
    );

    // Weight moved: the head is now inside the backed subtree, which the
    // tie-break alone would never have selected.
    let head = node.head();
    let carrier_root: Bytes32 = carrier.message.block.hash_tree_root().into();
    assert_eq!(
        head, carrier_root,
        "the head must descend into the backed sibling's subtree",
    );
    assert_ne!(
        head, fork.unbacked,
        "non-vacuity: the head is no longer the tie-break winner",
    );

    // The carrier's `proposer_attestation` is the invalid default, and it must
    // NOT have reached the pool — the fold deliberately ignores that field,
    // because it is covered by neither `block_root` nor `state_root` and a peer
    // can substitute it freely. The count asserted above already pins this: it
    // equals the number of BODY votes exactly, so a folded proposer attestation
    // would have pushed it one higher.
    assert_eq!(
        carrier.message.proposer_attestation,
        Attestation::default(),
        "fixture precondition: the carrier's proposer attestation is the invalid \
         default, so the count above would change if it were ever folded",
    );
}

#[test]
fn blocks_only_node_matches_gossip_node() {
    let fork = fork();
    let carrier = carrier_block(&fork);

    // Gossip node: sees the attestations over gossip AND the blocks.
    let gossip_node = engine_with_fork(&fork);
    for vote in &fork.votes {
        // `#[must_use]` — assert rather than discard.
        assert!(
            matches!(
                gossip_node.import_attestation(vote.clone()),
                AttestationImportResult::Accepted { .. }
            ),
            "gossip vote must be admitted on the gossip node",
        );
    }
    assert!(matches!(
        gossip_node.import_block(carrier.clone()),
        BlockImportResult::Accepted { .. }
    ));

    // Blocks-only node: never sees a gossip attestation. The same votes reach it
    // only inside the carrier block's body.
    let blocks_only_node = engine_with_fork(&fork);
    assert!(matches!(
        blocks_only_node.import_block(carrier),
        BlockImportResult::Accepted { .. }
    ));

    assert_eq!(
        blocks_only_node.head(),
        gossip_node.head(),
        "a node that receives only blocks must resolve the same head as one that \
         also receives the gossip attestations",
    );
    // Non-vacuity: both heads moved off the tie-break winner. Without this, two
    // nodes that both failed to count the votes would agree — and pass.
    assert_ne!(
        gossip_node.head(),
        fork.unbacked,
        "both nodes must have counted the votes, not merely agreed",
    );
}
