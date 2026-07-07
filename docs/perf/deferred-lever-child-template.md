# Deferred performance lever — child-issue template

> Copy this file into a new `perf/<lever-slug>` child issue when — and only when —
> the lever's trigger metric shows growth. The umbrella tracking issue stays open.

## Trigger-metric status (from the trigger-metric scaffolding)

| Lever | Hot path | Trigger metric | Wired at boundary? |
| ----- | -------- | -------------- | ------------------ |
| Incremental fork-choice | `forkchoice/src/helpers.rs:50` | `lean_fork_choice_block_processing_time_seconds` | YES (import + tick) |
| Bound the block/state tree (prune below finalized) | `forkchoice/src/store.rs:61,65` | same as above + process RSS | YES (metric); pruning safe only after finality advances |
| HTR memoization | `ssz/src/merkleize.rs:58` | `lean_state_transition_slots_processing_time_seconds` | NO — deferred: sub-phase inside `protocol::State::state_transition`; splitting it needs in-protocol timing, which the cross-cutting rule forbids. The child must decide how to expose this without violating the rule. `lean_state_transition_time_seconds` (whole transition) is the coarse proxy trigger. |
| Trim per-block `State` clone | `protocol/src/state.rs:774` | `lean_state_transition_time_seconds` (buckets to 4 s) | YES |

**Fork-choice observation coverage (intentional):** the
`lean_fork_choice_block_processing_time_seconds` histogram is observed at the **import**
(`accept_new_votes`) and **slot-tick** (`tick_interval`) paths — the recurring
per-slot/per-block cost the triggers care about. `produce_block` and
`produce_attestation_vote` also recompute the fork-choice head but are **not** observed:
they fire only when this node is the proposer/attester for the slot (a minority of slots),
so including them would skew the distribution the trigger reads. A child that needs
producer-path latency should add a separately-named histogram rather than fold it into
this one.

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
