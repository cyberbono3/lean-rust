# Changelog

<!--
Adding an entry: the PR that makes the change adds its own entry, in the same PR,
before it merges. Put one line under the matching `####` heading in the current
`## vX.Y.Z (Unreleased)` section:

  - [INTEROP] <Past-tense sentence naming the operator-visible effect> ([#N](https://github.com/cyberbono3/lean-rust/pull/N)).

Describe the effect an operator would notice, not the commit subject. End with an
explicit markdown link — a bare `(#N)` does not autolink in a rendered file. Cite the
driving issue when there is one (`/issues/N`), otherwise the PR (`/pull/N`). Prefix
`[INTEROP]` when the change alters something another client observes: wire or SSZ
layout, the state or `Config` shape, signature payload bytes, the leanSig revision,
genesis synthesis, or fork-choice head selection. Headings are `#### Features`,
`#### Changes`, `#### Fixes`; omit any with no entries. A PR that needs no entry
carries the `no changelog` label — `.github/workflows/changelog.yml` enforces this.
-->

## v0.1.0 (Unreleased)

#### Features

- Added the pq-devnet-0 node: block production, fork choice, SSZ containers, storage, and a single binary that runs a validating node ([#71](https://github.com/cyberbono3/lean-rust/pull/71)).
- Added a self-driving consensus loop, so a node advances slots and produces blocks from its own clock instead of an external driver ([#96](https://github.com/cyberbono3/lean-rust/pull/96)).
- Added a standalone single-node devnet mode with optional on-disk persistence, so a node can be restarted without losing its chain ([#103](https://github.com/cyberbono3/lean-rust/pull/103)).
- Added trigger-metric histograms covering the deferred-performance levers, exported on the existing metrics endpoint ([#105](https://github.com/cyberbono3/lean-rust/pull/105)).
- [INTEROP] Added leanSig signing: blocks and attestations now carry real post-quantum signatures instead of placeholder bytes, and per-validator one-time-signature key state is persisted at the sign boundary so a restarted node never reuses an epoch. Signature verification is implemented behind an injection seam but is not yet enabled on the import path ([#159](https://github.com/cyberbono3/lean-rust/pull/159)).
- [INTEROP] Added an offline genesis validator keygen CLI and the `genesis_validators.yaml` manifest format it produces ([#146](https://github.com/cyberbono3/lean-rust/pull/146)); the node-side loader that hydrates the genesis validator registry from that manifest, and therefore fixes the genesis state root, landed with the sign/verify stack ([#159](https://github.com/cyberbono3/lean-rust/pull/159)).
- Added architecture documentation with global and per-layer class and sequence diagrams ([#80](https://github.com/cyberbono3/lean-rust/pull/80)).
- Added this changelog and `scripts/check-changelog.sh`, which requires every pull request to record its change or carry the `no changelog` label ([#237](https://github.com/cyberbono3/lean-rust/issues/237)).

#### Changes

- [INTEROP] Pinned the leanSig dependency to an exact revision and added a guard that rejects a floating `branch` or `tag`, because a different revision produces signatures other clients do not verify ([#131](https://github.com/cyberbono3/lean-rust/pull/131)).
- Renamed the vote containers to the attestation family, aligning the type names with leanSpec; the gossip topic path is unchanged ([#138](https://github.com/cyberbono3/lean-rust/pull/138)).
- [INTEROP] Replaced `SignedBlock` with `SignedBlockWithAttestation`, moved per-vote signatures out of `BlockBody` into a sibling `BlockSignatures` list, and made block identity the inner `Block` hash-tree-root rather than the envelope root; the block wire layout and every block root a peer computes changed ([#141](https://github.com/cyberbono3/lean-rust/pull/141)).
- [INTEROP] Added the `Validator` registry to `State`, which changes the state SSZ layout and therefore every state root ([#142](https://github.com/cyberbono3/lean-rust/pull/142)).
- [INTEROP] Derived the validator count from the registry rather than from a `Config` field, changing the state `Config` shape and the roots computed over it ([#228](https://github.com/cyberbono3/lean-rust/pull/228)).
- Raised the workspace MSRV and the pinned toolchain to 1.87, the floor required by leanSig ([#130](https://github.com/cyberbono3/lean-rust/pull/130)).
- Moved six startup and shutdown log statements from tracing target `lean_rust` to `lean_cli::*`, so `RUST_LOG=lean_rust=debug` no longer selects them ([#236](https://github.com/cyberbono3/lean-rust/pull/236)).

#### Fixes

- [INTEROP] Broke equal-weight fork-choice head ties on `(weight, root)` rather than `(weight, slot, root)`, matching leanSpec; a node could previously select a different head than its peers ([#233](https://github.com/cyberbono3/lean-rust/pull/233)).
- [INTEROP] Fed block-carried attestations into fork choice on import; they were previously applied to state but never counted as votes, so head selection could diverge from peers that counted them ([#240](https://github.com/cyberbono3/lean-rust/pull/240)).
- [INTEROP] Kept incoming signed attestations byte-identical through the vote pool, instead of re-encoding them; a re-encoded attestation could fail verification at a peer ([#242](https://github.com/cyberbono3/lean-rust/pull/242)).
- [INTEROP] Rejected attestations whose head checkpoint fails the three predicates leanSpec asserts and this client had ignored: the head root must be tracked, its slot must be at or after the target's, and its declared slot must match its block's. A peer sending such an attestation now has it rejected rather than counted ([#244](https://github.com/cyberbono3/lean-rust/pull/244)).
- [INTEROP] Walked the attestation target back to a justifiable slot before voting, so this node now attests to a slot that may be older than the block it would previously have targeted; both walks are floored at the finalized checkpoint, a deliberate divergence from the reference's unbounded second loop ([#244](https://github.com/cyberbono3/lean-rust/pull/244)).

## Prior history

These notes start at [#71](https://github.com/cyberbono3/lean-rust/pull/71), the first
commit merged under the squash-with-PR-number convention. That is a scoping decision, not
a claim that the earlier record is unreadable: roughly thirty numbered pull requests before
it — the HTTP API, the metrics endpoint, the p2p stack, the key-generation CLI, the Docker
devnet — are recoverable from `git log` and their merge commits. They are omitted here to
bound the backfill, and can be added to these notes before the tag is cut if anyone wants
them.
