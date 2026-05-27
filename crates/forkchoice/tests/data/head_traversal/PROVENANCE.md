# head_traversal — provenance

LMD-GHOST head-traversal vectors backing `crates/forkchoice/tests/parity.rs`.

The upstream canonical fixture, `consensus/forkchoice/testdata/trajectory/baseline.json`, is a full-store trajectory (genesis → AdvanceTime → ImportBlock → ProcessAttestation → …). Replaying it requires the block-import path (`ImportBlock`), which lands in a later forkchoice change.

Until then, the vectors exercised by `parity.rs` are **hand-derived**:

- Linear chain, no votes → head defaults to the deepest reachable block.
- Two-fork supermajority → head follows weight.
- Tie-break by slot, then by root-bytes lex order.
- `min_score` threshold filters under-supported subtrees.
- Empty inputs and missing roots surface the typed `ForkchoiceError` variants.

When `ImportBlock` lands, the trajectory replay can be added as an extra parity test against `baseline.json`; the hand-derived cases stay in place as deterministic regression coverage for `get_fork_choice_head` in isolation.
