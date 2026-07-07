# Deferred performance lever — child-issue template

> Copy this file into a new `perf/<lever-slug>` child issue when — and only when —
> the lever's trigger metric shows growth. The umbrella tracking issue stays open.

## Trigger-metric status (from the trigger-metric scaffolding)

Hot-path references below name the file and symbol rather than a line number
(line numbers rot); `grep` the symbol to locate the current site.

| Lever | Hot path | Trigger metric | Wired at boundary? |
| ----- | -------- | -------------- | ------------------ |
| Incremental fork-choice | `forkchoice::helpers::get_fork_choice_head` | `lean_fork_choice_block_processing_time_seconds` | YES (import path) |
| Bound the block/state tree (prune below finalized) | `forkchoice::Store` block/state maps | same as above + process RSS | YES (metric); pruning safe only after finality advances |
| HTR memoization | `ssz::merkleize::merkleize` | `lean_state_transition_slots_processing_time_seconds` | NO — deferred: sub-phase inside `protocol::State::state_transition`; splitting it needs in-protocol timing, which the cross-cutting rule forbids. The child must decide how to expose this without violating the rule. `lean_state_transition_time_seconds` (whole transition) is the coarse proxy trigger. |
| Trim per-block `State` clone | `protocol::State::state_transition` (per-block clone) | `lean_state_transition_time_seconds` (buckets to 4 s) | YES |

**Fork-choice observation scope (intentional):** the
`lean_fork_choice_block_processing_time_seconds` histogram is observed **only** around the
isolated `accept_new_votes` recompute on the **block-import path** (`transition_and_track`).
It is deliberately NOT observed around `tick_interval`: that call bundles clock advance with
interval-boundary work and fires multiple times per slot (including near-no-op intervals), so
timing the whole call would mismeasure and dilute the distribution — and isolating the
recompute inside `tick_interval` would require timing within `forkchoice`, which the
cross-cutting rule forbids. `produce_block` / `produce_attestation_vote` also recompute the
head but are not observed (proposer/attester-only slots would skew the distribution). A child
that needs tick-path or producer-path latency should add a separately-named histogram rather
than fold it into this one.

## Per-child gate (all MUST hold before the child is "done")

1. **Prove the regression first.** Attach the trigger-metric graph/number showing growth
   BEFORE any code change. No proactive optimization (anti-goal).
2. **Scaffolding only.** No consensus-semantic change; the lever is an internal
   representation/caching change, not spec logic.
3. **Cross-cutting rule.** No metrics/logging/time/RNG inside `protocol/` or
   `forkchoice/` transition code — instrument at the chain-tick boundary only.
4. **Dependency check.** All levers are post-loop. Pruning below finalized is safe ONLY
   once finality advances — confirm before implementing.

## Acceptance criteria (per child)

- [ ] Trigger metric measurably regressed before work started (evidence attached).
- [ ] Consensus spec-test vector outputs unchanged.
- [ ] Metric improves after the change; `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green.
